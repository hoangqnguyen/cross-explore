# Builds the desktop app on Linux to prove the codebase compiles there
# (WebKitGTK webview, inotify watcher, Secret Service keychain).
#   docker build -f docker/linux-build/Dockerfile -t cx-linux-build . && docker run --rm cx-linux-build
FROM rust:1.92-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
      libssl-dev libdbus-1-dev libxdo-dev pkg-config build-essential curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN mkdir -p app/dist && echo '<!doctype html>' > app/dist/index.html
CMD cargo build --release -p cross-explore && ls -la target/release/cross-explore && cargo test --workspace --exclude cross-explore 2>&1 | grep -E "test result|FAILED|panicked" 
