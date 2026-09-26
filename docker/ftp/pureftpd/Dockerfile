# Pure-FTPd with MLSD/MLST and explicit TLS, built natively for the host's
# architecture (the popular prebuilt images are amd64-only and crash under
# emulation on Apple silicon).
# (Alpine's pure-ftpd is a minimal build without FEAT/MLSD/TLS; Debian's is full.)
FROM debian:bookworm-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends pure-ftpd openssl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -M -s /usr/sbin/nologin ftpuser
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
EXPOSE 21 30000-30019
ENTRYPOINT ["/entrypoint.sh"]
