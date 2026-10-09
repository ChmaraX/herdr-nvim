# syntax=docker/dockerfile:1.7
#
# hnv-box: golden image for reproducing/verifying herdr-nvim bugs.
# Real herdr + herdr-nvim (built from a mounted worktree) + real pi agent
# talking to a local scripted mock LLM, recorded with VHS. Runs offline.
#
# Build with `tests/e2e/box build` (it passes the build contexts/args).
# Layers are ordered by change frequency: OS/tools -> precompiled crate
# deps -> herdr -> box scripts -> pi (last, so a "latest pi" rebuild is cheap).
#
# Build contexts:
#   .    = tests/e2e (this directory)
#   src  = a herdr-nvim checkout (only Cargo.toml + Cargo.lock are read)

ARG RUST_IMAGE=rust:1-bookworm

# ---------------------------------------------------------------- manifests
# Normalize the root package version so a release bump does not invalidate
# the (slow) precompiled-deps layer below.
FROM debian:bookworm-slim AS manifests
COPY --from=src Cargo.toml Cargo.lock /m/
RUN sed -i '0,/^version = .*/s//version = "0.0.0"/' /m/Cargo.toml \
 && awk 'BEGIN{p=0} /^name = "herdr-nvim"$/{p=1; print; next} p==1 && /^version = /{print "version = \"0.0.0\""; p=0; next} {p=0; print}' /m/Cargo.lock > /m/Cargo.lock.n \
 && mv /m/Cargo.lock.n /m/Cargo.lock

# ---------------------------------------------------------------- base OS
FROM ${RUST_IMAGE} AS base
ARG TARGETARCH
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      chromium ffmpeg git curl ca-certificates xz-utils procps psmisc less jq \
      ripgrep fd-find file locales tini \
      fonts-jetbrains-mono fonts-dejavu-core fonts-noto-color-emoji \
 && sed -i 's/^# *en_US.UTF-8/en_US.UTF-8/' /etc/locale.gen && locale-gen \
 && rm -rf /var/lib/apt/lists/*
ENV LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 TERM=xterm-256color

# ttyd (VHS needs >= 1.7.2; bookworm has none)
ARG TTYD_VERSION=1.7.7
RUN case "$TARGETARCH" in arm64) a=aarch64;; amd64) a=x86_64;; *) echo "unsupported arch $TARGETARCH" >&2; exit 1;; esac \
 && curl -fsSL -o /usr/local/bin/ttyd "https://github.com/tsl0922/ttyd/releases/download/${TTYD_VERSION}/ttyd.${a}" \
 && chmod +x /usr/local/bin/ttyd && ttyd --version

# VHS
ARG VHS_VERSION=0.12.1
RUN case "$TARGETARCH" in arm64) a=arm64;; amd64) a=x86_64;; esac \
 && curl -fsSL "https://github.com/charmbracelet/vhs/releases/download/v${VHS_VERSION}/vhs_${VHS_VERSION}_Linux_${a}.tar.gz" | tar xz -C /tmp \
 && install -m755 /tmp/vhs_*/vhs /usr/local/bin/vhs && rm -rf /tmp/vhs_* && vhs --version

# Neovim (official tarball; herdr-nvim needs >= 0.10, bookworm ships 0.7)
ARG NVIM_VERSION=v0.12.5
RUN case "$TARGETARCH" in arm64) a=arm64;; amd64) a=x86_64;; esac \
 && curl -fsSL "https://github.com/neovim/neovim/releases/download/${NVIM_VERSION}/nvim-linux-${a}.tar.gz" | tar xz -C /opt \
 && ln -s /opt/nvim-linux-${a}/bin/nvim /usr/local/bin/nvim && nvim --version | head -1

# Node 24
ARG NODE_MAJOR=24
RUN case "$TARGETARCH" in arm64) a=arm64;; amd64) a=x64;; esac \
 && f=$(curl -fsSL "https://nodejs.org/dist/latest-v${NODE_MAJOR}.x/SHASUMS256.txt" | awk -v a="linux-${a}.tar.xz" '$2 ~ a"$" {print $2}') \
 && curl -fsSL "https://nodejs.org/dist/latest-v${NODE_MAJOR}.x/${f}" | tar xJ -C /usr/local --strip-components=1 \
 && node --version

# Unprivileged user; owns the cargo home so runtime builds can write locks.
RUN useradd -m -u 1000 -s /bin/bash box \
 && mkdir -p /work /cargo-target /proof /opt/herdr-nvim \
 && chown -R box:box /usr/local/cargo /work /cargo-target /proof /opt/herdr-nvim
ENV CARGO_TARGET_DIR=/cargo-target CARGO_NET_OFFLINE=true \
    VHS_NO_SANDBOX=true SHELL=/bin/bash
USER box
WORKDIR /home/box

# ---------------------------------------------------------------- precompiled deps (cargo-chef style)
# Build a dummy crate with the real manifests at the SAME path (/work) and
# profile (release) the runtime uses, so a runtime build only compiles the
# herdr-nvim crate itself. /cargo-target seeds each box's named volume.
COPY --from=manifests --chown=box:box /m/Cargo.toml /m/Cargo.lock /work/
RUN cd /work && mkdir -p src examples \
 && echo 'fn main() {}' > src/main.rs && cp src/main.rs examples/fff_cold.rs \
 && CARGO_NET_OFFLINE=false cargo fetch --locked \
 && cargo build --release --locked --offline \
 && rm -rf /cargo-target/release/herdr-nvim* /cargo-target/release/deps/herdr_nvim-* \
           /cargo-target/release/.fingerprint/herdr-nvim-* \
 && rm -rf /work/* && du -sh /cargo-target /usr/local/cargo/registry

# ---------------------------------------------------------------- herdr
ARG HERDR_VERSION=0.9.3
RUN case "$TARGETARCH" in arm64) a=aarch64;; amd64) a=x86_64;; esac \
 && if [ "$HERDR_VERSION" = latest ]; then u="https://github.com/herdrdev/herdr/releases/latest/download/herdr-linux-${a}"; \
    else u="https://github.com/herdrdev/herdr/releases/download/v${HERDR_VERSION#v}/herdr-linux-${a}"; fi \
 && mkdir -p /home/box/.local/bin && curl -fsSL -o /home/box/.local/bin/herdr "$u" \
 && chmod +x /home/box/.local/bin/herdr && /home/box/.local/bin/herdr --version
ENV PATH=/home/box/.local/bin:/opt/box/bin:$PATH

# ---------------------------------------------------------------- box runtime
COPY --chown=box:box image/ /opt/box/
RUN /opt/box/bin/box-seed

# ---------------------------------------------------------------- pi (LAST)
ARG PI_VERSION=latest
USER root
RUN npm install -g --no-fund --no-audit "@earendil-works/pi-coding-agent@${PI_VERSION}" && npm cache clean --force
USER box
ENV PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
RUN herdr integration install pi \
 && /opt/box/bin/box-versions > /tmp/v && cat /tmp/v
USER root
RUN mv /tmp/v /etc/box-versions
USER box

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/opt/box/bin/box-init"]
