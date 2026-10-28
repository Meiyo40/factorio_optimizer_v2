# The Kotlin build's own Java 25 image (.devcontainer) plus a Rust toolchain, so the JVM baseline and the Rust port
# run on the same CPU in the same container.
FROM mcr.microsoft.com/devcontainers/java:25-trixie

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential valgrind libfreetype6 fontconfig fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target GRADLE_USER_HOME=/cache/gradle
ENV PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | CARGO_HOME=/opt/cargo sh -s -- -y --profile minimal --default-toolchain stable \
    && mkdir -p /cache /work \
    && chown -R vscode:vscode /cache /work /opt/rustup /opt/cargo

USER vscode
WORKDIR /work
