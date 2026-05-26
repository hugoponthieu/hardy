# Rebuilding The RISC-V `libcsp` SDK For `hardy`

This guide explains what you need to rebuild on the build machine when
`hardy-bpa-server` is meant to run inside a RISC-V VM.

The short version:

- The files in `hardy/libcsp/lib/*.a` are build-time inputs, not VM runtime files.
- Copying those `.a` files into the VM does not help the binary start.
- If `hardy-bpa-server` still reports `/lib/ld.so.1` or `libstdc++.so.6`, the
  final binary is still dynamically linked from the VM's point of view.
- To improve portability, rebuild the native SDK in the sibling source repo
  `/home/hugo/code/libcsp`, then rebuild `hardy-bpa-server` against that SDK,
  then inspect the final ELF again.

## What Lives Where

`/home/hugo/code/hardy/libcsp` is only an SDK drop:

- `include/`
- `lib/`

It is not the native source tree and does not contain the real rebuild logic.

The actual source/build repo is:

- `/home/hugo/code/libcsp`

That repo already contains a RISC-V build flow and packaging scripts.

## What Must Be Rebuilt

For the current `grpc,cspcl` build, the important native archives are:

- `libcsp.a`
- `libzmq.a`
- `libsocketcan.a`
- `libbz2.a`

These archives must all be built for the same target ABI and toolchain family
as the final Rust binary. In the current setup that means:

- target family: `riscv64-linux-gnu`
- Rust target: `riscv64gc-unknown-linux-gnu`
- linker/toolchain family: `riscv64-linux-gnu-*`

`libzmq.a` is especially important because it is a C++ archive and is the most
likely reason the final link still pulls in C++ and glibc runtime requirements.

## Host Requirements

The `libcsp` repo's existing RISC-V scripts expect these tools on the build
machine:

- `riscv64-linux-gnu-gcc`
- `riscv64-linux-gnu-g++`
- `riscv64-linux-gnu-ar`
- `riscv64-linux-gnu-ranlib`
- `riscv64-linux-gnu-strip`
- `pkg-config`
- `cmake`
- `autoreconf`
- `make`
- `git`

The documented Arch packages in `/home/hugo/code/libcsp/INSTALL.rst` start
with:

```bash
sudo pacman -S --needed \
    riscv64-linux-gnu-gcc \
    riscv64-linux-gnu-binutils \
    riscv64-linux-gnu-glibc
```

The scripted flow may also install other host-side tooling through:

- `/home/hugo/code/libcsp/install-riscv-full.sh`
- `/home/hugo/code/libcsp/scripts/riscv-install/00-install-host-tools.sh`

## Recommended Rebuild Flow

Run the rebuild in the native source repo, not in `hardy/libcsp`.

### 1. Build The Native SDK

From `/home/hugo/code/libcsp`:

```bash
./install-riscv-full.sh
```

This existing script runs the full sequence:

- `scripts/riscv-install/10-build-bzip2.sh`
- `scripts/riscv-install/20-build-libsocketcan.sh`
- `scripts/riscv-install/30-build-libzmq.sh`
- `scripts/riscv-install/40-verify-deps.sh`
- `scripts/riscv-install/50-build-libcsp.sh`
- `scripts/riscv-install/60-package-artifact.sh`

It builds a dedicated RISC-V dependency prefix, then packages a staged SDK.

### 2. Use The Staged SDK Output

After a successful build, the important outputs are in:

- `/home/hugo/code/libcsp/.riscv-deps/lib/*.a`
- `/home/hugo/code/libcsp/build-riscv64/libcsp.a`
- `/home/hugo/code/libcsp/artifacts/libcsp-riscv64-linux-gnu/`
- `/home/hugo/code/libcsp/artifacts/libcsp-v1.6-riscv64-linux-gnu.tar.gz`

Prefer the staged SDK directory or packaged tarball:

- `/home/hugo/code/libcsp/artifacts/libcsp-riscv64-linux-gnu/`

Do not mix a random old `lib/` directory with newly generated headers.

### 3. Refresh The SDK Copy Used By `hardy`

Update the SDK drop inside `hardy` from the staged artifact:

- replace `hardy/libcsp/include/`
- replace `hardy/libcsp/lib/`

The key idea is that `hardy/libcsp` should be a clean copy of the SDK produced
by the `libcsp` rebuild, not a hand-curated mix of files from different builds.

### 4. Rebuild `hardy-bpa-server`

From `hardy`, rebuild the RISC-V binary with the existing wrapper:

```bash
./scripts/build-riscv-bpa-cspcl.sh dynamic
```

or, if you want the script to reject a misleading non-portable output:

```bash
./scripts/build-riscv-bpa-cspcl.sh portable
```

`portable` mode is strict. It will fail if the final ELF still contains:

- a requested program interpreter such as `/lib/ld.so.1`
- shared-library requirements such as `libstdc++.so.6`

## What Portability Means Here

Rebuilding the `.a` files is necessary, because they are part of the final link.
But rebuilding them is not automatically sufficient to produce a single-file
portable binary.

Current observed behavior for `hardy-bpa-server` is:

- interpreter: `/lib/ld.so.1`
- needed shared library: `libstdc++.so.6`

That means the VM still needs the matching runtime for the current artifact.

This is why copying `.a` files into the VM does not solve the runtime error.
The VM only cares about the final executable and whatever runtime loader/shared
libraries that executable still expects.

### Important Caveat From `libcsp`

The `libcsp` documentation already notes that its current `waf` discovery uses:

- `pkg-config --libs`

not:

- `pkg-config --static`

That means the native build flow does not guarantee that every transitive
static-link dependency will be carried all the way into the final application
link for you.

Even after rebuilding the SDK, the final `hardy-bpa-server` binary may still be
dynamic if the toolchain, `libzmq.a`, or the final link step resolves against
shared runtime pieces.

## Validation Checklist

After rebuilding the SDK, verify the native outputs first:

```bash
ls /home/hugo/code/libcsp/artifacts/libcsp-riscv64-linux-gnu/lib/libcsp.a
ls /home/hugo/code/libcsp/artifacts/libcsp-riscv64-linux-gnu/lib/libzmq.a
ls /home/hugo/code/libcsp/artifacts/libcsp-riscv64-linux-gnu/lib/libsocketcan.a
ls /home/hugo/code/libcsp/artifacts/libcsp-riscv64-linux-gnu/lib/libbz2.a
```

Then rebuild `hardy-bpa-server` and inspect the resulting ELF:

```bash
file target/riscv64gc-unknown-linux-gnu/release/hardy-bpa-server
readelf -l target/riscv64gc-unknown-linux-gnu/release/hardy-bpa-server | grep interpreter
readelf -d target/riscv64gc-unknown-linux-gnu/release/hardy-bpa-server | grep NEEDED
```

Interpret the results like this:

- If `readelf -l` shows `/lib/ld.so.1`, the binary still needs a guest-side
  runtime loader.
- If `readelf -d` shows `libstdc++.so.6`, the binary still needs the shared C++
  runtime in the VM.
- If both are absent, you are much closer to a self-contained artifact.

## What To Do On The VM

Do not copy `.a` files into the VM to run the server.

Use one of these two models:

- Dynamic model:
  install the runtime expected by the final binary in the VM
- Portable model:
  rebuild the SDK and final binary until the ELF no longer depends on the VM's
  loader/shared runtime

For the current failure:

```text
-bash: ./hardy-bpa-server: cannot execute: required file not found
```

the missing file is typically not the server binary itself. It is the ELF
interpreter embedded in the binary, which in the current build is:

```text
/lib/ld.so.1
```
