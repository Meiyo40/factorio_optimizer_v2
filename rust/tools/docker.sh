#!/usr/bin/env bash
# Runs the Kotlin baseline and the Rust port in one Linux container (Java 25 + Rust), for golden files and timings.
#
#   rust/tools/docker.sh build            build the image
#   rust/tools/docker.sh sync             copy the repo into the container's work volume
#   rust/tools/docker.sh run '<command>'  run a command in /work (after sync)
#   rust/tools/docker.sh out <dir>        copy /work/out from the volume into <dir>
#
# The committed tree is exported with `git archive` (LF line endings; a Windows checkout has CRLF, which breaks
# gradlew), then this working tree's rust/ and the Kotlin helpers in rust/tools/kotlin/ are laid over it. The
# Kotlin sources in the repo are never changed. Gradle's and Cargo's caches live in the factorio-cache volume.
set -euo pipefail
export MSYS_NO_PATHCONV=1   # Git Bash must not rewrite container paths
repo="$(cd "$(dirname "$0")/../.." && pwd)"
winrepo="$(cd "$repo" && { pwd -W 2>/dev/null || pwd; })"   # docker.exe wants C:/..., not /c/...
image=factorio-baseline
mounts=(-v factorio-cache:/cache -v factorio-work:/work)

case "${1:-}" in
  build)
    docker build -t "$image" "$winrepo/rust/tools"
    ;;
  sync)
    cd "$repo"
    git archive HEAD | docker run --rm -i -v factorio-work:/work "$image" tar -xf - -C /work
    tar --exclude=rust/target -cf - rust | docker run --rm -i -v factorio-work:/work "$image" tar -xf - -C /work
    tar -C rust/tools/kotlin -cf - . | docker run --rm -i -v factorio-work:/work "$image" tar -xf - -C /work/src/test/kotlin
    # The committed gradlew has CRLF line endings, which Linux cannot execute.
    docker run --rm -v factorio-work:/work "$image" sed -i 's/\r$//' /work/gradlew
    ;;
  run)
    shift
    docker run --rm "${mounts[@]}" -w /work "$image" bash -c "$*"
    ;;
  out)
    mkdir -p "$2"
    docker run --rm "${mounts[@]}" "$image" tar -cf - -C /work/out . | tar -xf - -C "$2"
    ;;
  *)
    sed -n '2,9p' "$0"
    exit 1
    ;;
esac
