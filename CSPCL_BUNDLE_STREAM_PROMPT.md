Update the upstream Rust `cspcl` binding so Hardy can use a bundle stream safely, without adding any `unsafe` code to the Hardy repo.

Context:

- Repo using the binding: `/home/hugo/code/hardy`
- Upstream binding repo: `/home/hugo/code/cspcl/rust-bindings/cspcl`
- Current Hardy integration file: `/home/hugo/code/hardy/cspcl/src/cla.rs`

Current problem:

- The upstream binding exposes `bundle_stream(&mut self) -> BundleStream<'_>`.
- `BundleStream` holds an exclusive mutable borrow of `Cspcl` for the whole lifetime of the stream.
- Hardy’s CLA needs to both:
  - receive bundles continuously
  - send bundles concurrently
- Hardy currently stores one shared `Arc<Mutex<Cspcl>>` and uses that same instance for both RX and TX.
- If Hardy creates `bundle_stream()` from the locked `Cspcl`, the mutex guard stays held for the stream lifetime.
- Then `forward()` cannot lock the same `Cspcl` to call `send_bundle()`.
- Result: sends stall and no CAN traffic appears on `vcan`.

Additional constraint:

- Do not add `unsafe` code to the Hardy repo.
- If `unsafe` is required internally, it must remain encapsulated inside the upstream `cspcl` crate.
- Prefer reducing or eliminating `unsafe` in the binding if possible.

Important observation:

- The current upstream `bundle_stream` implementation already uses a background thread and raw-pointer `unsafe` to keep calling `recv_bundle(100)`.
- This API shape is fundamentally incompatible with a transport object that also needs concurrent sending through the same `Cspcl` instance.

Required goal:

Design and implement a safe upstream API in the `cspcl` crate that allows:

1. continuous inbound bundle reception as a stream or receiver
2. concurrent outbound bundle sending through a separate sender/handle
3. a single internal owner of the mutable `Cspcl`
4. no new `unsafe` in Hardy

Preferred architecture:

- Introduce an owned driver abstraction in the upstream `cspcl` crate.
- That driver should take ownership of the `Cspcl` instance and run one internal worker task/thread.
- The worker is the only place allowed to mutate `Cspcl`.
- The worker should:
  - poll `recv_bundle(timeout)` regularly
  - also drain/process outbound send requests from a channel
- The public API should expose something like:

```rust
let driver = Cspcl::new(...)?; // or CspclDriver::new(...)
let (tx, rx) = driver.start();
```

Where:

- `tx` is an async or thread-safe sending handle for outbound bundles
- `rx` is a stream/receiver of inbound bundles

Example target API ideas:

```rust
pub struct BundleDriver { ... }
pub struct BundleSender { ... }

impl Cspcl {
    pub fn into_bundle_driver(self) -> BundleDriver;
}

impl BundleDriver {
    pub fn start(self) -> (BundleSender, impl Stream<Item = Result<Bundle>>);
}

impl BundleSender {
    pub async fn send_bundle(&self, data: Vec<u8>, dest_addr: u8, dest_port: u8) -> Result<()>;
}
```

Alternative acceptable design:

- `BundleDriver::split()` returning:
  - `BundleSink`
  - `BundleStream`
- internally both communicate with one worker that owns `Cspcl`

Implementation requirements:

1. Keep the public API safe.
2. Keep all transport mutation serialized through one owner.
3. Do not require callers to hold `&mut Cspcl` for the entire stream lifetime.
4. Avoid exposing raw pointers or borrow-tied stream lifetimes in the public API.
5. Make shutdown behavior explicit and clean.
6. Preserve current send/recv semantics and error reporting as much as practical.
7. Add or update tests for the new driver API if the upstream repo has a test setup for this area.
8. Update docs/comments for the new API.

Then update Hardy to use the new upstream API:

- Modify `/home/hugo/code/hardy/cspcl/src/cla.rs`
- Replace direct shared use of `Arc<Mutex<Cspcl>>` with the new safe driver handles from upstream
- Listener should consume the inbound stream/receiver
- `forward()` should use the outbound sender/handle
- No `unsafe` in Hardy

Also clean up the temporary wrong direction if present:

- Remove any direct `cspcl_sys` usage from Hardy CLA code
- Hardy should only use safe APIs from the `cspcl` crate

Deliverables:

1. Upstream `cspcl` API change implementing the owned driver/channel model
2. Hardy CLA updated to use that API
3. Brief explanation of why `bundle_stream(&mut self)` was incompatible with Hardy’s shared transport model
4. Notes on shutdown/error propagation behavior

Please be careful not to “fix” this by:

- adding more `Mutex` layering around the same borrowed stream
- keeping a lock held for the full lifetime of the receive stream
- using `unsafe` in Hardy
- duplicating `Cspcl` instances unless you have confirmed the underlying CSP stack supports that correctly

If you consider using two separate `Cspcl` instances for TX and RX, treat that as a last resort and justify it carefully, because `cspcl_init` also configures global CSP stack/router state and may not be safely repeatable per logical endpoint.
