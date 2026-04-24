# Build And Publish A RISC-V BPA With CSPCL

This guide builds a `hardy-bpa-server` package for
`riscv64gc-unknown-linux-gnu` with the built-in `cspcl` CLA enabled.

The release model is:

- `libcsp` is built and distributed separately as its own tarball
- Hardy links against that unpacked `libcsp` tree
- Hardy is packaged as one clean `.tar.gz` file
- the tarball is published through GitHub Releases so other machines can pull it

## What You Produce

- one separate `libcsp` tarball for `riscv64 GNU`
- one Hardy tarball containing:
  - `hardy-bpa-server`
  - `example_config.yaml`
  - `README-install.md`

## Prerequisites

Install these on the build host:

- Rust with the `riscv64gc-unknown-linux-gnu` target
- `riscv64-linux-gnu-gcc`
- `clang`
- `protobuf`

Example on Arch Linux:

```bash
rustup target add riscv64gc-unknown-linux-gnu

sudo pacman -Syu --needed \
  clang \
  protobuf \
  riscv64-linux-gnu-gcc
```

## 1. Prepare The Separate libcsp Tarball

Build `libcsp` outside this repository and archive the source tree together
with the RISC-V build output.

Hardy expects the unpacked tarball to look like this:

```text
libcsp/
  include/
  lib/
    libcsp.a
```

Create the tarball from the parent directory of `libcsp/`:

```bash
tar -czf libcsp-riscv64-gnu.tar.gz libcsp
```

## 2. Unpack libcsp For The Hardy Build

```bash
mkdir -p /tmp/libcsp-input
tar -xzf libcsp-riscv64-gnu.tar.gz -C /tmp/libcsp-input
```

Set the cross linker and the CSP paths:

```bash
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc
export CSP_REPO_DIR=/tmp/libcsp-input/libcsp
export CSP_BUILD_DIR=/tmp/libcsp-input/libcsp/lib
```

## 3. Build The Hardy Binary

```bash
cargo build --release \
  -p hardy-bpa-server \
  --bin hardy-bpa-server \
  --target riscv64gc-unknown-linux-gnu \
  --no-default-features \
  --features grpc,cspcl
```

Confirm the binary exists:

```bash
ls -l target/riscv64gc-unknown-linux-gnu/release/hardy-bpa-server
```

## 4. Create A Nice Tarball

Create a staging directory and assemble the release contents:

```bash
VERSION=0.1.0
TARGET=riscv64gc-unknown-linux-gnu
ASSET_DIR=/tmp/hardy-bpa-server-${VERSION}-${TARGET}

rm -rf "${ASSET_DIR}"
mkdir -p "${ASSET_DIR}"

cp target/${TARGET}/release/hardy-bpa-server "${ASSET_DIR}/hardy-bpa-server"
cp bpa-server/example_config.yaml "${ASSET_DIR}/example_config.yaml"
cp RELEASE_RISCV_CSPCL.md "${ASSET_DIR}/README-install.md"
```

Then archive it:

```bash
tar -C /tmp -czf /tmp/hardy-bpa-server-${VERSION}-${TARGET}.tar.gz \
  hardy-bpa-server-${VERSION}-${TARGET}
```

Inspect the tarball:

```bash
tar -tzf /tmp/hardy-bpa-server-${VERSION}-${TARGET}.tar.gz
```

## 5. Publish To GitHub Releases

The common distribution path is a GitHub Release.

Recommended release flow:

1. Upload the separate `libcsp` tarball to a stable URL or keep it available in
   your internal release process.
2. Build the Hardy tarball as shown above.
3. Create a Git tag such as `v0.1.0`.
4. Create a GitHub Release and upload:
   - `hardy-bpa-server-0.1.0-riscv64gc-unknown-linux-gnu.tar.gz`
   - your separate `libcsp-riscv64-gnu.tar.gz`

Using the GitHub CLI:

```bash
git tag v0.1.0
git push origin v0.1.0

gh release create v0.1.0 \
  /tmp/hardy-bpa-server-0.1.0-riscv64gc-unknown-linux-gnu.tar.gz \
  ./libcsp-riscv64-gnu.tar.gz \
  --title "v0.1.0" \
  --notes "RISC-V GNU Hardy BPA server with CSPCL support."
```

## 6. Install On Another Machine

Download and unpack the Hardy tarball:

```bash
tar -xzf hardy-bpa-server-0.1.0-riscv64gc-unknown-linux-gnu.tar.gz
cd hardy-bpa-server-0.1.0-riscv64gc-unknown-linux-gnu
chmod +x hardy-bpa-server
```

Use `example_config.yaml` as the starting point for your deployment
configuration.

If your runtime environment needs additional `libcsp` platform setup, install
that separately according to how you built and deployed `libcsp`.

## 7. Verify Basic Startup

First confirm the binary starts:

```bash
./hardy-bpa-server --help
```

Then create a CSPCL-enabled config. A minimal CLA block looks like this:

```yaml
node-ids: "ipn:1.0"

storage:
  metadata:
    type: memory
  bundle:
    type: memory

grpc:
  address: "[::1]:50051"
  services: ["application", "service", "cla"]

clas:
  - name: csp-uplink
    type: cspcl
    local-addr: 1
    port: 10
    interface: can
    interface-name: can0
    peers:
      - node-id: "ipn:2.0"
        addr: 2
        port: 10
```

Start the server:

```bash
./hardy-bpa-server --config ./config.yaml
```

For end-to-end forwarding validation after installation, continue with
[Two-node CSPCL](cspcl-two-node.md).

## Troubleshooting

- If bindgen cannot find CSP headers, confirm `CSP_REPO_DIR/include` exists.
- If linking fails, confirm `CSP_BUILD_DIR/libcsp.a` exists.
- If the server starts but CSPCL cannot initialize, verify the underlying
  `libcsp` runtime setup for your selected interface mode.
