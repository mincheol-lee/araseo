# Araseo repository instructions

## Required verification

- After changing Rust production code, Slint UI code, build configuration, or
  runtime scripts, run `./scripts/verify` before reporting completion.
- Changes that affect the UI, Windows integration, dependencies, or a release
  executable must also pass `./scripts/verify --windows`.
- Do not treat compilation alone as verification when a headless behavioral
  test can cover the change. Add or update a Harness test for regressions.
- Keep tests pointed at the production modules under `src/`; do not copy their
  implementation into the Harness.

## Local Rust and Windows toolchain

- Cargo is installed at `/home/minch/.cargo/bin/cargo`, but it may not be on
  `PATH` in a fresh Codex shell. Before running repository scripts, load it
  with `source /home/minch/.cargo/env`.
- Windows builds from WSL use the Rust target `x86_64-pc-windows-gnu` and a
  locally extracted MinGW toolchain. First look for
  `/tmp/araseo-mingw/usr/bin/x86_64-w64-mingw32-dlltool`; a previous session
  may already have prepared it.
- When the local toolchain exists, ensure the generic compiler name is present:

  ```sh
  ln -sfn x86_64-w64-mingw32-gcc-posix \
    /tmp/araseo-mingw/usr/bin/x86_64-w64-mingw32-gcc
  ```

- `/tmp` is not persistent. If the toolchain is missing, recreate it without a
  system-wide install. Download these Ubuntu packages into
  `/tmp/araseo-mingw-pkgs`: `binutils-mingw-w64-x86-64`,
  `gcc-mingw-w64-base`, `gcc-mingw-w64-x86-64-posix`,
  `gcc-mingw-w64-x86-64-posix-runtime`, `mingw-w64-common`, and
  `mingw-w64-x86-64-dev`. Extract every `.deb` into
  `/tmp/araseo-mingw` using `dpkg-deb -x`, then create the compiler symlink
  above. Package downloads may require network approval; do not request a
  system-wide `sudo apt install` when local extraction is sufficient.

  ```sh
  mkdir -p /tmp/araseo-mingw-pkgs /tmp/araseo-mingw
  cd /tmp/araseo-mingw-pkgs
  apt-get download \
    binutils-mingw-w64-x86-64 \
    gcc-mingw-w64-base \
    gcc-mingw-w64-x86-64-posix \
    gcc-mingw-w64-x86-64-posix-runtime \
    mingw-w64-common \
    mingw-w64-x86-64-dev
  for package_path in /tmp/araseo-mingw-pkgs/*.deb; do
    dpkg-deb -x "$package_path" /tmp/araseo-mingw
  done
  ln -sfn x86_64-w64-mingw32-gcc-posix \
    /tmp/araseo-mingw/usr/bin/x86_64-w64-mingw32-gcc
  cd /home/minch/agent_ide
  ```
- For commands that need the cross-toolchain, prepend
  `/tmp/araseo-mingw/usr/bin` to `PATH` after loading Cargo:

  ```sh
  source /home/minch/.cargo/env
  export PATH=/tmp/araseo-mingw/usr/bin:$PATH
  ```

## Verification in this WSL environment

- Run the normal verification first:

  ```sh
  bash -c 'source /home/minch/.cargo/env; exec ./scripts/verify'
  ```

- This sandbox can expose `WSL_DISTRO_NAME` while denying the Windows host-disk
  lookup. If and only if the normal run fails solely in
  `wsl_diagnostics::tests::collects_a_real_read_only_shell_probe` with
  `Could not locate the drive storing this distribution's WSL VHDX`, record
  that raw failure and rerun the deterministic suite with that one environment
  marker removed:

  ```sh
  env -u WSL_DISTRO_NAME bash -c \
    'source /home/minch/.cargo/env; exec ./scripts/verify'
  ```

- Run Windows verification with the local MinGW tools:

  ```sh
  env -u WSL_DISTRO_NAME bash -c \
    'source /home/minch/.cargo/env; export PATH=/tmp/araseo-mingw/usr/bin:$PATH; exec ./scripts/verify --windows'
  ```

## Building a Windows test executable

- From WSL, do not use `scripts/build-windows.ps1` unless Windows itself has a
  working Rust installation. Build with the verified WSL cross-toolchain:

  ```sh
  bash -c \
    'source /home/minch/.cargo/env; export PATH=/tmp/araseo-mingw/usr/bin:$PATH; exec cargo build --locked --release --target x86_64-pc-windows-gnu'
  cp -f target/x86_64-pc-windows-gnu/release/araseo.exe dist/araseo.exe
  ```

- Confirm that the copied file is the new Windows binary:

  ```sh
  file dist/araseo.exe
  sha256sum target/x86_64-pc-windows-gnu/release/araseo.exe dist/araseo.exe
  /tmp/araseo-mingw/usr/bin/x86_64-w64-mingw32-objdump -p dist/araseo.exe \
    | rg 'DLL Name|Subsystem'
  ```

- `dist/` is intentionally ignored by Git. A successful test build therefore
  does not appear in `git status`.
