#!/bin/sh
# Build WaveTag for Windows (VST3 + CLAP) inside Docker.
# Output: target/bundled/WaveTag.vst3 and WaveTag.clap
set -e
cd "$(dirname "$0")"
docker build -q -t kicknull-build -f Dockerfile.build . >/dev/null
mkdir -p "$HOME/.cache/kicknull-cargo"
# target/ may be a symlink to other storage (e.g. a NAS); mount its real path too
TMNT=""; [ -L target ] && TMNT="-v $(readlink -f target):$(readlink -f target)"
docker run --rm -u "$(id -u):$(id -g)" \
  -e CARGO_HOME=/cargo -v "$HOME/.cache/kicknull-cargo:/cargo" \
  -v "${NICE_PLUG_AWK:-$(cd .. && pwd)/nice-plug-awk}:/nice-plug-awk:ro" -v "$PWD:/src" $TMNT -w /src -e HOME=/tmp kicknull-build \
  sh -c 'cargo xtask bundle wave_tag --release --target x86_64-pc-windows-gnu'
ls -la target/bundled
