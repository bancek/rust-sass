# Ecosystem gate: drop-in libsass testing against third-party bindings.
#
# Build from the repository root:
#   docker build -f rust-sass-libsass-tests/ecosystem/Dockerfile .               # everything (target `all`)
#   docker build -f rust-sass-libsass-tests/ecosystem/Dockerfile --target lua .  # one binding
# Or use run.sh for per-target PASS/FAIL reporting.
#
# Layout: libsass-builder (Ubuntu 24.04, release libsass) -> base (installed
# lib + headers + libsass.pc) -> one <lang>-smoke stage per binding, each
# ending in a compile-string assertion + `touch /tmp/ok` -> per-lang receipt
# finals (scratch + /tmp/ok) -> `all` aggregator. The build IS the gate:
# any failing smoke fails its stage.

# syntax=docker/dockerfile:1

ARG UBUNTU=24.04
ARG RUST_TOOLCHAIN=1.92

# --- builder ---
FROM ubuntu:${UBUNTU} AS libsass-builder
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential curl ca-certificates git pkg-config \
    && rm -rf /var/lib/apt/lists/*
ARG RUST_TOOLCHAIN
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "${RUST_TOOLCHAIN}"
ENV PATH="/root/.cargo/bin:${PATH}"
WORKDIR /src
COPY . .
# The libsass submodule must be checked out (headers source); everything else
# may be empty submodule dirs — cargo does not need them for this crate.
RUN test -f libsass/include/sass/context.h \
        || (echo "error: libsass submodule missing — run 'git submodule update --init libsass'" >&2; exit 1) \
    && cargo build --release -p rust-sass-libsass
RUN mkdir -p /out/lib /out/include \
    && cp target/release/libsass.so target/release/libsass.a /out/lib/ \
    && cp -r libsass/include/sass.h libsass/include/sass2scss.h libsass/include/sass /out/include/

# --- base ---
FROM ubuntu:${UBUNTU} AS base
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential curl ca-certificates git pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY --from=libsass-builder /out/lib/libsass.so /out/lib/libsass.a /usr/local/lib/
COPY --from=libsass-builder /out/include/ /usr/local/include/
RUN mkdir -p /usr/local/lib/pkgconfig && printf '%s\n' \
    'prefix=/usr/local' \
    'exec_prefix=${prefix}' \
    'libdir=${prefix}/lib' \
    'includedir=${prefix}/include' \
    'Name: libsass' \
    'Description: libsass ABI implemented on rust-sass' \
    'Version: 3.6.6' \
    'Cflags: -I${includedir}' \
    'Libs: -L${libdir} -lsass' \
    >/usr/local/lib/pkgconfig/libsass.pc \
    && ldconfig
# Smoke helpers live in the repo so they are reviewable without Docker.
COPY rust-sass-libsass-tests/ecosystem/smokes/ /smokes/

# --- python ---
FROM base AS python-smoke
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    python3 python3-pip python3-dev \
    && rm -rf /var/lib/apt/lists/* \
    && pip3 install --break-system-packages -q setuptools
RUN git clone https://github.com/sass/libsass-python /tmp/libsass-python \
    && git -C /tmp/libsass-python checkout -q b951b4b
WORKDIR /tmp/libsass-python
RUN SYSTEM_SASS=1 python3 setup.py build_ext --inplace \
    && PYTHONPATH=/tmp/libsass-python python3 /smokes/python/smoke.py \
    && touch /tmp/ok

# --- go ---
# Two bindings, one stage: wellington/go-libsass and its maintained fork
# bep/golibsass. Both use `-tags dev` for system libsass (default builds
# vendored C++). Go comes from the official tarball (pinned, arch-safe).
FROM base AS go-smoke
RUN curl -fsSL "https://go.dev/dl/go1.24.0.linux-$(dpkg --print-architecture).tar.gz" \
    | tar -xz -C /usr/local
ENV PATH="/usr/local/go/bin:${PATH}"
RUN git clone https://github.com/wellington/go-libsass /tmp/go-libsass \
    && git -C /tmp/go-libsass checkout -q e1cda02 \
    && git clone https://github.com/bep/golibsass /tmp/golibsass \
    && git -C /tmp/golibsass checkout -q 034cf88
RUN mkdir -p /tmp/gosmoke && cp /smokes/go/wellington/* /tmp/gosmoke/ \
    && cd /tmp/gosmoke && go mod tidy && go run -tags dev .
RUN mkdir -p /tmp/bepsmoke && cp /smokes/go/bep/* /tmp/bepsmoke/ \
    && cd /tmp/bepsmoke && go mod tidy && go run -tags dev . \
    && touch /tmp/ok

# --- ruby ---
FROM base AS ruby-smoke
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ruby ruby-dev \
    && rm -rf /var/lib/apt/lists/* \
    && gem install ffi --no-document
RUN git clone https://github.com/sass/sassc-ruby /tmp/sassc-ruby \
    && git -C /tmp/sassc-ruby checkout -q eddbacb \
    && cp /usr/local/lib/libsass.so /tmp/sassc-ruby/lib/sassc/libsass.so
WORKDIR /tmp/sassc-ruby
RUN ruby -Ilib /smokes/ruby/smoke.rb && touch /tmp/ok

# --- perl ---
FROM base AS perl-smoke
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    perl make cpanminus \
    && rm -rf /var/lib/apt/lists/* \
    && cpanm --notest -q ExtUtils::CppGuess Capture::Tiny Encode::Locale Filesys::Notify::Simple
RUN git clone https://github.com/sass/perl-libsass /tmp/perl-libsass \
    && git -C /tmp/perl-libsass checkout -q c63c21c \
    && git -C /tmp/perl-libsass submodule update --init --depth 1 libsass
WORKDIR /tmp/perl-libsass
# The binding unconditionally compiles vendored C++; empty @SOURCES and link
# the system lib instead (XS glue uses only the public C API).
RUN patch -p1 </smokes/perl/system-libsass.patch \
    && perl Makefile.PL --no-plugins --skip-git && make \
    && perl -Mblib /smokes/perl/smoke.pl \
    && touch /tmp/ok

# --- dotnet ---
FROM base AS dotnet-smoke
RUN curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
    && bash /tmp/dotnet-install.sh --channel 8.0 --install-dir /usr/share/dotnet \
    && ln -s /usr/share/dotnet/dotnet /usr/bin/dotnet
ENV DOTNET_ROOT=/usr/share/dotnet PATH="/usr/share/dotnet:${PATH}" DOTNET_CLI_TELEMETRY_OPTOUT=1
# No ICU on minimal Ubuntu; invariant globalization is fine for ASCII asserts.
ENV DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1
RUN git clone https://github.com/sass/libsass-net /tmp/libsass-net \
    && git -C /tmp/libsass-net checkout -q ce9661c
RUN mkdir -p /tmp/cssmoke && cp /smokes/dotnet/* /tmp/cssmoke/ \
    && cd /tmp/cssmoke && dotnet build -v q \
    && cp /usr/local/lib/libsass.so "$(dirname "$(find bin -name cssmoke.dll | head -1)")/libsass64.so" \
    && dotnet "$(find bin -name cssmoke.dll | head -1)" \
    && touch /tmp/ok

# --- java ---
# Pure-Gradle recipe: jsass resolves its own deps (no coordinates in this
# file). We only rebuild the JNI glue against our static libsass and run
# Smoke.java via the `smoke` task from smokes/java/smoke.init.gradle.
FROM base AS java-smoke
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    openjdk-11-jdk-headless \
    && rm -rf /var/lib/apt/lists/*
# openjdk-11 satisfies the build's Java toolchain (11) locally, so the
# foojay resolver does not provision one over the network.
RUN git clone https://github.com/bit3/jsass /tmp/jsass \
    && git -C /tmp/jsass checkout -q ebad4f4
# JNI glue links our static libsass; overwriting the checked-in prebuilt for
# this arch means processResources puts it on the classpath as before, so no
# binding sources change.
RUN PLAT="linux-x64"; [ "$(dpkg --print-architecture)" = arm64 ] && PLAT="linux-aarch64" \
    && JH="$(dirname "$(find /usr/lib/jvm -name jni.h | head -1)")" \
    && cc -O2 -fPIC -shared -I"$JH" -I"$JH/linux" -I/usr/local/include \
        /tmp/jsass/src/main/c/io_bit3_jsass_adapter_NativeAdapter.c \
        /usr/local/lib/libsass.a -lstdc++ -o "/tmp/jsass/src/main/resources/${PLAT}/libjsass.so"
# --write-locks: jsass enables dependency locking with no committed lockfile.
# `smoke` stays clear of `jar`/`test`/`buildNative*` (see smoke.init.gradle).
RUN cd /tmp/jsass && ./gradlew --no-daemon --write-locks -I /smokes/java/smoke.init.gradle smoke \
    && touch /tmp/ok

# --- lua ---
FROM base AS lua-smoke
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    lua5.4 liblua5.4-dev \
    && rm -rf /var/lib/apt/lists/*
RUN git clone https://github.com/craigbarnes/lua-sass /tmp/lua-sass \
    && git -C /tmp/lua-sass checkout -q c983211
WORKDIR /tmp/lua-sass
# 2018-era -Werror vs Lua 5.4 headers: relax warnings. Then relink: the
# Makefile's flag order (-lsass before sass.o) silently drops DT_NEEDED.
RUN make all LUA_PC=lua5.4 CFLAGS="-g -O2 -Wall" \
    && cc -shared -o sass.so sass.o /usr/local/lib/libsass.so \
    && make check LUA_PC=lua5.4 LUA=lua5.4 CFLAGS="-g -O2 -Wall" \
    && LUA_CPATH="./?.so" lua5.4 /smokes/lua/smoke.lua \
    && touch /tmp/ok

# --- php ---
FROM base AS php-smoke
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    php php-dev \
    && rm -rf /var/lib/apt/lists/*
RUN git clone https://github.com/sensational/sassphp /tmp/sassphp \
    && git -C /tmp/sassphp checkout -q 261c6af \
    && mkdir -p /tmp/sassphp/lib/libsass/lib /tmp/sassphp/lib/libsass/include \
    && cp -r /usr/local/include/sass.h /usr/local/include/sass2scss.h \
        /usr/local/include/sass /tmp/sassphp/lib/libsass/include/ \
    && cp /usr/local/lib/libsass.so /tmp/sassphp/lib/libsass/lib/
WORKDIR /tmp/sassphp
RUN phpize && ./configure -q && make -s \
    && php -dextension=/tmp/sassphp/modules/sass.so /smokes/php/smoke.php \
    && touch /tmp/ok

# --- rust ---
FROM base AS rust-smoke
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain 1.92
ENV PATH="/root/.cargo/bin:${PATH}"
# sass-sys build.rs prefers system libsass via pkg-config when present.
RUN git clone https://github.com/compass-rs/sass-rs /tmp/sass-rs \
    && git -C /tmp/sass-rs checkout -q fb1e42a
WORKDIR /tmp/sass-rs
RUN cargo test && cargo build -q \
    && ./target/debug/sass-rs </smokes/rust/smoke.scss | grep -q "color: red" \
    && touch /tmp/ok

# --- nim ---
# No arm64 binaries and choosenim rejects arm64 — source build (arch-safe).
FROM base AS nim-smoke
ARG NIM_VERSION=2.2.4
RUN git clone --depth 1 --branch "v${NIM_VERSION}" https://github.com/nim-lang/Nim /tmp/Nim \
    && cd /tmp/Nim && sh build_all.sh \
    && ln -s /tmp/Nim/bin/nim /usr/local/bin/nim \
    && nim --version
RUN git clone https://github.com/dom96/sass /tmp/sass \
    && git -C /tmp/sass checkout -q c466216
RUN nim c --path:/tmp/sass/src -o:/tmp/nimsmoke /smokes/nim/smoke.nim \
    && /tmp/nimsmoke \
    && cd /tmp/sass && nim c -r tests/tester \
    && touch /tmp/ok

# --- node ---
FROM base AS node-smoke
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*
# Env BEFORE npm install so no prebuilt binary is even attempted.
ENV SKIP_SASS_BINARY_DOWNLOAD_FOR_CI=1 SASS_FORCE_BUILD=1 LIBSASS_EXT=yes \
    CPLUS_INCLUDE_PATH=/usr/local/include LIBSASS_CFLAGS="" LIBSASS_LDFLAGS="" \
    LIBSASS_LIBRARY="-L/usr/local/lib -Wl,-rpath,/usr/local/lib -lsass"
RUN git clone https://github.com/sass/node-sass /tmp/node-sass \
    && git -C /tmp/node-sass checkout -q 107ded94
WORKDIR /tmp/node-sass
RUN npm install --no-audit --no-fund && node scripts/build.js --force \
    && node /smokes/node/smoke.js \
    && touch /tmp/ok

# --- sassc ---
# The reference C CLI, compiled against our headers/lib (mirrors
# tool/build-sassc.sh; the harness wrapper that forces `-t expanded` is a
# host-gate concern — here we pass --style explicitly).
FROM base AS sassc-smoke
RUN git clone https://github.com/sass/sassc /tmp/sassc \
    && git -C /tmp/sassc checkout -q a92527c
WORKDIR /tmp/sassc
RUN SASSC_VERSION="$(git describe --tags 2>/dev/null || echo rust-sass-libsass)" \
    && cc -Wall -O2 -DSASSC_VERSION="\"$SASSC_VERSION\"" -I/usr/local/include \
        -c sassc.c -o sassc.o \
    && cc -o sassc sassc.o -L/usr/local/lib -lsass -Wl,-rpath,/usr/local/lib \
    && ./sassc -v \
    && sh /smokes/sassc/smoke.sh ./sassc \
    && touch /tmp/ok

# --- receipts ---
FROM scratch AS python
COPY --from=python-smoke /tmp/ok /python-ok
FROM scratch AS go
COPY --from=go-smoke /tmp/ok /go-ok
FROM scratch AS ruby
COPY --from=ruby-smoke /tmp/ok /ruby-ok
FROM scratch AS perl
COPY --from=perl-smoke /tmp/ok /perl-ok
FROM scratch AS dotnet
COPY --from=dotnet-smoke /tmp/ok /dotnet-ok
FROM scratch AS java
COPY --from=java-smoke /tmp/ok /java-ok
FROM scratch AS lua
COPY --from=lua-smoke /tmp/ok /lua-ok
FROM scratch AS php
COPY --from=php-smoke /tmp/ok /php-ok
FROM scratch AS rust
COPY --from=rust-smoke /tmp/ok /rust-ok
FROM scratch AS nim
COPY --from=nim-smoke /tmp/ok /nim-ok
FROM scratch AS node
COPY --from=node-smoke /tmp/ok /node-ok
FROM scratch AS sassc
COPY --from=sassc-smoke /tmp/ok /sassc-ok

# --- all ---
# Default target: builds everything, collects all receipts.
FROM scratch AS all
COPY --from=python /python-ok /python-ok
COPY --from=go /go-ok /go-ok
COPY --from=ruby /ruby-ok /ruby-ok
COPY --from=perl /perl-ok /perl-ok
COPY --from=dotnet /dotnet-ok /dotnet-ok
COPY --from=java /java-ok /java-ok
COPY --from=lua /lua-ok /lua-ok
COPY --from=php /php-ok /php-ok
COPY --from=rust /rust-ok /rust-ok
COPY --from=nim /nim-ok /nim-ok
COPY --from=node /node-ok /node-ok
COPY --from=sassc /sassc-ok /sassc-ok
