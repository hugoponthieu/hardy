#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_triple="riscv64gc-unknown-linux-gnu"
binary_name="hardy-bpa-server"
binary_path="${repo_root}/target/${target_triple}/release/${binary_name}"

mode="${HARDY_RISCV_BUILD_MODE:-portable}"
if [[ "${1:-}" == "portable" || "${1:-}" == "dynamic" ]]; then
  mode="$1"
  shift
fi

linker="${CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER:-riscv64-linux-gnu-g++}"
export CSP_REPO_DIR="${CSP_REPO_DIR:-${repo_root}/libcsp}"
export CSP_BUILD_DIR="${CSP_BUILD_DIR:-${repo_root}/libcsp/lib}"
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER="${linker}"

required_static_libs=(
  "${CSP_BUILD_DIR}/libcsp.a"
  "${CSP_BUILD_DIR}/libzmq.a"
  "${CSP_BUILD_DIR}/libsocketcan.a"
  "${CSP_BUILD_DIR}/libbz2.a"
)

cargo_args=(
  build
  --release
  -p "${binary_name}"
  --bin "${binary_name}"
  --target "${target_triple}"
  --no-default-features
  --features "grpc,cspcl"
)

die() {
  echo "error: $*" >&2
  exit 1
}

info() {
  echo "==> $*"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

require_file() {
  [[ -f "$1" ]] || die "required file not found: $1"
}

toolchain_file() {
  local name="$1"
  "${linker}" --print-file-name="${name}"
}

append_rustflag() {
  rustflags+=("-C" "link-arg=$1")
}

append_codegen_flag() {
  rustflags+=("-C" "$1")
}

check_portable_toolchain() {
  local missing=()
  local path

  for lib in libstdc++.a libgcc.a libgcc_eh.a; do
    path="$(toolchain_file "${lib}")"
    if [[ -z "${path}" || "${path}" == "${lib}" || ! -f "${path}" ]]; then
      missing+=("${lib}")
    fi
  done

  if ((${#missing[@]} > 0)); then
    die "portable mode requires static cross-toolchain runtimes (${missing[*]}). Install a RISC-V static libstdc++/libgcc toolchain or use dynamic mode."
  fi
}

inspect_binary() {
  local file_output interp needed summary

  require_file "${binary_path}"

  file_output="$(file "${binary_path}")"
  interp="$(
    readelf -l "${binary_path}" \
      | sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p'
  )"
  needed="$(
    readelf -d "${binary_path}" 2>/dev/null \
      | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p'
  )"

  if [[ -n "${interp}" ]]; then
    summary="dynamic"
  else
    summary="static"
  fi

  echo
  info "artifact summary"
  echo "mode=${mode}"
  echo "target=${target_triple}"
  echo "linker=${linker}"
  echo "artifact=${binary_path}"
  echo "linkage=${summary}"
  echo "${file_output}"
  if [[ -n "${interp}" ]]; then
    echo "interpreter=${interp}"
  else
    echo "interpreter=<none>"
  fi
  if [[ -n "${needed}" ]]; then
    echo "needed:"
    while IFS= read -r lib; do
      [[ -n "${lib}" ]] && echo "  ${lib}"
    done <<< "${needed}"
  else
    echo "needed=<none>"
  fi

  if [[ "${mode}" == "portable" ]]; then
    [[ -z "${interp}" ]] || die "portable build still requires the ELF interpreter ${interp}"
    [[ -z "${needed}" ]] || die "portable build still depends on shared libraries: $(paste -sd ', ' <<< "${needed}")"
  fi
}

require_command cargo
require_command file
require_command readelf
require_command "${linker}"

[[ -d "${CSP_REPO_DIR}/include" ]] || die "required include directory not found: ${CSP_REPO_DIR}/include"
for lib in "${required_static_libs[@]}"; do
  require_file "${lib}"
done

rustflags=()
if [[ -n "${CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_RUSTFLAGS:-}" ]]; then
  # Preserve any caller-provided flags while allowing this script to add stricter link args.
  # shellcheck disable=SC2206
  rustflags=(${CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_RUSTFLAGS})
fi

case "${mode}" in
  portable)
    append_codegen_flag "target-feature=+crt-static"
    check_portable_toolchain
    append_rustflag "-static-libstdc++"
    append_rustflag "-static-libgcc"
    append_rustflag "-Wl,--start-group"
    append_rustflag "-lstdc++"
    append_rustflag "-lsupc++"
    append_rustflag "-lc"
    append_rustflag "-lm"
    append_rustflag "-lpthread"
    append_rustflag "-ldl"
    append_rustflag "-lutil"
    append_rustflag "-lrt"
    append_rustflag "-lgcc_eh"
    append_rustflag "-lgcc"
    append_rustflag "-Wl,--end-group"
    ;;
  dynamic)
    append_rustflag "-lstdc++"
    append_rustflag "-lsupc++"
    ;;
  *)
    die "unknown mode '${mode}'. Use 'portable' or 'dynamic'."
    ;;
esac

export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_RUSTFLAGS="${rustflags[*]}"

info "building ${binary_name}"
echo "mode=${mode}"
echo "CSP_REPO_DIR=${CSP_REPO_DIR}"
echo "CSP_BUILD_DIR=${CSP_BUILD_DIR}"
echo "LINKER=${linker}"
echo "RUSTFLAGS=${CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_RUSTFLAGS}"

cargo "${cargo_args[@]}" "$@"
inspect_binary
