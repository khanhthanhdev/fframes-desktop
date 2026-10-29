# Self-hosted runners

The desktop checks run on two self-hosted runners, one Ubuntu and one Windows, in
[Desktop self-hosted validation](../../.github/workflows/desktop-self-hosted.yml). Both keep
their build folder between runs, so only changed crates compile.

While the desktop code is being refactored, every push runs only the fast checks. The heavy
tests run when you start the workflow by hand with **full** ticked; run them before a release:

```sh
gh workflow run "Desktop self-hosted validation" -R khanhthanhdev/fframes-desktop --ref <branch> -f full=true
```

## What runs where

| Check | Where | When |
| --- | --- | --- |
| fmt, tests, clippy, protocol/runtime tests, packaging and release gate tests | self-hosted Linux and Windows | every push |
| SDK and Windows test build | self-hosted Windows | pushes to `main`, full runs |
| Real worker, SDK, managed, portable, preview, timeline, audio parity | self-hosted Linux and Windows | full runs |
| X11 shell tests (the real app on Xvfb, driven with xdotool) | self-hosted Linux | full runs |
| Linux test build | self-hosted Linux | full runs |
| fmt, tests, clippy for pull requests from other repositories | hosted Ubuntu ([Desktop CI](../../.github/workflows/desktop.yml)) | those pull requests |
| tests and clippy on macOS (`desktop-macos`) | hosted `macos-15` (Desktop CI) | pull requests that touch desktop paths, and **Run workflow** |
| dependency sources and advisories (`cargo deny`) | hosted Ubuntu (Desktop CI) | pull requests that touch desktop paths, and **Run workflow** |

There is no self-hosted macOS runner. macOS is covered only by the hosted Desktop CI job; it is not
checked by hand, and [Desktop Release](../../.github/workflows/desktop-release.yml) builds Windows only.

The repository is public, so the self-hosted workflow runs only on `push` and **Run workflow**.
Never add a `pull_request` trigger to it: anyone could then run code on these machines, as root.
A branch you push gets the self-hosted checks whether or not it has a pull request.

## Ubuntu runner

Use Ubuntu 24.04 (x86-64). An admin installs the packages once; packages installed with `apt`
are system-wide, so the runner user sees them.

1. Install the packages as root:

   ```sh
   sudo apt-get update
   sudo apt-get install -y build-essential cmake pkg-config git nasm clang libclang-dev \
     libssl-dev libfontconfig1-dev libasound2-dev libxkbcommon-x11-dev libwayland-dev \
     libegl1-mesa-dev libgl1-mesa-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev \
     libx11-xcb-dev libzstd-dev mesa-vulkan-drivers xvfb xdotool python3 curl unzip
   ```

   - `nasm`, `make`, `gcc` and `git` build the pinned static FFmpeg (once per change of
     `desktop/scripts/build-native-ffmpeg.py`).
   - `xvfb` and `xdotool` run the X11 shell tests on a virtual display.

2. Create the runner user and register the runner. In the repository, open **Settings →
   Actions → Runners → New self-hosted runner → Linux x64** and follow the download and
   `./config.sh` steps as that user. Keep the default labels `self-hosted`, `Linux` and `X64`.
   Then install it as a service, as root, from the runner folder:

   ```sh
   sudo ./svc.sh install <runner-user>
   sudo ./svc.sh start
   ```

   Rust needs no manual install: the job installs the pinned toolchain for the runner user.

3. The runner user needs no sudo. Earlier versions of this workflow ran an isolated-account
   check as root; a runner set up for it can have `/etc/sudoers.d/github-runner` removed.

4. Check the machine as the runner user. This is the same check the job runs first; it
   prints nothing when everything is there:

   ```sh
   for tool in nasm cmake make git pkg-config gcc python3 Xvfb xdotool; do
     command -v "$tool" >/dev/null || echo "missing: $tool"
   done
   for package in libclang-dev libssl-dev libasound2-dev; do
     dpkg -s "$package" >/dev/null 2>&1 || echo "missing: $package"
   done
   ```

   If a package is missing, the job stops at **Check Linux native prerequisites** and prints the
   `apt-get install` line to run.

## Windows runner

The Windows job expects, for the runner's service account: `rustup` in `%USERPROFILE%\.cargo\bin`,
LLVM in `C:\Program Files\LLVM` (for `libclang.dll`), Python 3 reachable through the `py`
launcher, and the Visual Studio C++ build tools. It downloads the pinned FFmpeg build itself and
checks its SHA-256. Register it with the labels `self-hosted`, `Windows` and `X64`.

## Disk and housekeeping

Both runners keep state in the runner's tool cache folder (`<runner>/_work/_tool` on Linux):

- `fframes-target`: the build folder. It grows with every toolchain or dependency change. When
  disk runs low, delete it; the next run rebuilds everything once.
- `fframes-ffmpeg/<key>` (Linux): the FFmpeg install for one version of its build script. Old
  keys can be deleted.
- `fframes-ffmpeg/<distribution>` (Windows): the pinned FFmpeg download, checked against its
  SHA-256. Old distributions can be deleted.

## Test builds

Test builds are kept for 14 days:

- `studio-test-x86_64-pc-windows-msvc`: the packaged Windows app with the SDK, from every push to
  `main` and every full run.
- `studio-test-x86_64-unknown-linux-gnu`: the packaged Linux app with the SDK, plus the audio
  parity evidence, from full runs only.

The run's summary page links the zip directly and shows a `gh run download` command for it. To
download the newest Windows test build from `main`:

```sh
gh run download "$(gh run list -R khanhthanhdev/fframes-desktop -w "Desktop self-hosted validation" -b main -s success -L 1 --json databaseId -q '.[0].databaseId')" -R khanhthanhdev/fframes-desktop -n studio-test-x86_64-pc-windows-msvc -D studio-test
```

A branch has a test build only after a full run on it: replace `main` with the branch name and
add `-e workflow_dispatch` to pick the newest full run (also for the Linux build on `main`). Test
builds are named
`studio-test-*` so a release never picks them up: [Desktop Release](../../.github/workflows/desktop-release.yml)
collects only `fframes-studio-*` packages, which are built on hosted runners.
