#!/bin/sh
# Build WaveTag for Windows (VST3 + CLAP) inside Docker.
# Output: target/bundled/WaveTag.vst3 and WaveTag.clap
set -e
cd "$(dirname "$0")"
docker build -q -t kicknull-build -f Dockerfile.build . >/dev/null
mkdir -p "$HOME/.cache/kicknull-cargo"
docker run --rm -u "$(id -u):$(id -g)" \
  -e CARGO_HOME=/cargo -v "$HOME/.cache/kicknull-cargo:/cargo" \
  -v "${NIH_PLUG_AWK:-$(cd .. && pwd)/nih-plug-awk}:/nih-plug-awk:ro" -v "$PWD:/src" -w /src -e HOME=/tmp kicknull-build \
  sh -c 'cargo xtask bundle wave_tag --release --target x86_64-pc-windows-gnu'
ls -la target/bundled
