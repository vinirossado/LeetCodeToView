#!/bin/bash
# Builds two minimal, isolated root filesystems (one per sandboxed language) that
# nsjail chroots into instead of reusing the whole container's "/" (see java.rs's
# and csharp.rs's --chroot flag, and tasks.md "Filesystem temporário/efêmero").
# Before this, --chroot / meant the jailed process could read the ENTIRE
# container filesystem read-only, including /etc/shadow (confirmed via
# test-snippets/FilesystemEscape.java) -- this script builds a real, separate
# root containing ONLY what each runtime actually needs.
#
# WHY a script instead of inline Dockerfile RUN lines: this needs loops/variables
# to stay portable across CPU architectures (see the MULTIARCH detection below) --
# baking arch-specific paths directly into a Dockerfile would repeat the same
# amd64/arm64 hardcoding this project has already been burned by elsewhere (see
# java.rs/csharp.rs's own "derived on linux/arm64, not verified on amd64"
# caveats -- this script doesn't fix that caveat, it just avoids adding a NEW
# one on top). Shared verbatim between Dockerfile.api and .ci/Dockerfile (both
# COPY + RUN this same file) instead of duplicating the block in each, since
# both stage the exact same JDK/dotnet installs beforehand.
#
# File list derived EMPIRICALLY, not guessed: `strace -f -e trace=openat,open,
# stat,lstat,newfstatat,statx,access,faccessat,readlink,readlinkat` on the real
# `java ... Debugger <class>` / `sandbox-runner --csharp-worker --dll <dll>`
# command lines (same methodology already used to derive JAVA_SECCOMP_POLICY/
# CSHARP_SECCOMP_POLICY's syscall sets in java.rs/csharp.rs -- see those
# constants' doc comments -- just tracking file PATHS here instead of syscall
# names), run OUTSIDE any jail (same reasoning as the seccomp derivation: javac
# runs outside the jail entirely, and stracing the raw command line is what
# actually shows which files the runtime touches, before nsjail's own R/O
# chroot masks failures as "would have been blocked anyway"), across every
# snippet in sandbox/test-snippets/ (13 files) and sandbox/test-snippets-csharp/
# (11 projects, plus a StringVar re-check specifically for ICU/globalization
# usage -- see the ICU comment below). See tasks.md "Filesystem temporário/
# efêmero" for the full writeup of what was found and what's still open.
set -euo pipefail

OUT="${1:?usage: build-minimal-rootfs.sh <output-base-dir>}"
JAVA_ROOT="$OUT/java"
CSHARP_ROOT="$OUT/csharp"
RUBY_ROOT="$OUT/ruby"
DOTNET_VER="8.0.30" # keep in sync with the SDK channel installed above (dotnet-install.sh --channel 8.0) -- NOT derived automatically, see the "known limitation" note in tasks.md

# Multiarch dir (aarch64-linux-gnu / x86_64-linux-gnu) detected instead of
# hardcoded -- see this file's header comment on why.
MULTIARCH="$(ls /lib | grep -E '^(aarch64|x86_64)-linux-gnu$')"
JDK_HOME="$(readlink -f /usr/bin/java | sed 's#/bin/java##')" # resolves the update-alternatives symlink to the real Temurin install dir (e.g. /usr/lib/jvm/temurin-25-jdk-arm64)
# Ruby paths asked FROM Ruby itself (RbConfig), same "detect, don't
# hardcode a version" discipline as MULTIARCH/JDK_HOME above -- the Debian
# package installs Ruby 3.1 today (`ruby3.1`, pulled in by the plain
# `ruby` metapackage in the Dockerfile), but hardcoding "3.1.0" here would
# silently go stale the next time the base image's Ruby version bumps.
RUBY_BIN="$(readlink -f /usr/bin/ruby)" # resolves ruby -> ruby3.1 to the real binary
RUBY_LIBDIR="$(ruby -e 'puts RbConfig::CONFIG["rubylibdir"]')"     # pure-Ruby stdlib, e.g. /usr/lib/ruby/3.1.0
RUBY_ARCHDIR="$(ruby -e 'puts RbConfig::CONFIG["archdir"]')"       # compiled C extensions (json/ext, objspace.so, ...), e.g. /usr/lib/aarch64-linux-gnu/ruby/3.1.0
RUBY_VENDORLIBDIR="$(ruby -e 'puts RbConfig::CONFIG["vendorlibdir"]')"
RUBY_VENDORARCHDIR="$(ruby -e 'puts RbConfig::CONFIG["vendorarchdir"]')"
RUBY_ABI_VERSION="$(ruby -e 'puts RbConfig::CONFIG["ruby_version"]')" # e.g. "3.1.0" -- used only for the gems/specifications/default path below

echo "[minimal-rootfs] multiarch=$MULTIARCH jdk_home=$JDK_HOME dotnet_ver=$DOTNET_VER ruby_bin=$RUBY_BIN out=$OUT"

rm -rf "$JAVA_ROOT" "$CSHARP_ROOT" "$RUBY_ROOT"
mkdir -p "$JAVA_ROOT" "$CSHARP_ROOT" "$RUBY_ROOT"

# ─── Java rootfs ────────────────────────────────────────────────────────────
mkdir -p "$JAVA_ROOT/usr/lib/jvm" "$JAVA_ROOT/usr/bin" "$JAVA_ROOT/app" \
         "$JAVA_ROOT/tmp" "$JAVA_ROOT/proc" "$JAVA_ROOT/sys" "$JAVA_ROOT/etc" \
         "$JAVA_ROOT/lib/$MULTIARCH" "$JAVA_ROOT/usr/lib/$MULTIARCH/gconv" \
         "$JAVA_ROOT/workdir"
# /workdir: FIXED mount point the real (per-execution) workdir gets
# bind-mounted onto at runtime (java.rs's JAIL_WORKDIR) -- pre-created here,
# at image-build time, specifically because it CAN'T be created on demand at
# runtime: nsjail mounts the chroot root read-only before processing
# --bindmount_ro entries, so mkdir-ing a not-yet-existing mount target inside
# an already-read-only root fails (found empirically -- see java.rs's
# JAIL_WORKDIR comment for the exact error). A fixed, pre-created path
# sidesteps this; the dynamic per-execution directory name never needs to
# exist inside the image itself.

# Whole JDK tree copied wholesale, not file-by-file: the strace derivation
# confirmed the running JVM touches files scattered across bin/ and lib/,
# INCLUDING glibc's own HWCAP-based dynamic-linker subdirectory probing
# (bin/aarch64/libjli.so, bin/tls/atomics/libjli.so, etc. -- real search
# paths the loader tries, not separate artifacts) and the CDS archive
# (lib/server/classes.jsa, required -- see JAVA_SECCOMP_POLICY's `newfstat`
# comment in java.rs for how that dependency was originally found). All of it
# already lives inside this one self-contained directory; copying file-by-file
# would mean re-deriving this exact list by hand on every JDK bump, and would
# still miss the loader's own multi-path probing above. Copying the tree keeps
# it correct by construction while still cutting out everything OUTSIDE the
# JDK -- no /etc/shadow, no other apt packages, no other users' source code.
cp -a "$JDK_HOME" "$JAVA_ROOT/usr/lib/jvm/$(basename "$JDK_HOME")"
# Absolute symlink target, not relative: this is resolved INSIDE the jail at
# runtime (after chroot), where "/usr/lib/jvm/..." is correct as-is. A first
# cut of this script used a relative target here and got it wrong (resolved
# relative to /usr/bin/, i.e. to the nonexistent /usr/bin/usr/lib/jvm/...) --
# found empirically, running the real jail and seeing execve('/usr/bin/java')
# fail with ENOENT despite the file existing; `readlink` inside the built
# rootfs confirmed the bad relative target before this fix.
ln -s "/usr/lib/jvm/$(basename "$JDK_HOME")/bin/java" "$JAVA_ROOT/usr/bin/java"
# Dropped: man pages / legal texts / C headers -- confirmed empirically that a
# *running* JVM never opens anything under these (they don't appear anywhere
# in the strace output); only javac (which runs OUTSIDE this jail -- see
# java.rs::run's doc comment) or a human reading docs would need them.
rm -rf "$JAVA_ROOT"/usr/lib/jvm/*/man "$JAVA_ROOT"/usr/lib/jvm/*/legal "$JAVA_ROOT"/usr/lib/jvm/*/include

# jdi-driver.jar/.jsa: the JDI driver's own classpath (a jar, not the old
# exploded jdi-out/ dir) plus its CDS (Class Data Sharing) archive -- see
# Dockerfile.api's comment on the RUN step that generates these (right
# after the `javac ... Debugger.java` step, in the sandbox-build stage) and
# java.rs's DRIVER_CDS_JAR/DRIVER_CDS_ARCHIVE for how they're referenced at
# runtime. Staged at the SAME /app/... path they already live at in this
# image (not renamed) -- CDS validates the archive's app-classpath entry by
# exact path string, so this has to match exactly what java.rs passes as
# -cp / -XX:SharedArchiveFile.
cp -a /app/jdi-driver.jar "$JAVA_ROOT/app/jdi-driver.jar"
cp -a /app/jdi-driver.jsa "$JAVA_ROOT/app/jdi-driver.jsa"

# System glibc libs the `java`/libjli.so/libjvm.so ELF binaries themselves
# DT_NEED (confirmed via `ldd`, not guessed -- the JDK's OWN .so files, e.g.
# libjava.so/libjvm.so/libnet.so/libjdwp.so, are already inside the tree
# copied above; only the base glibc pieces below are external to it).
for lib in libc.so.6 libdl.so.2 libm.so.6 libpthread.so.0 librt.so.1 ld-linux-*.so.1; do
  cp -a "/lib/$MULTIARCH/"$lib "$JAVA_ROOT/lib/$MULTIARCH/" 2>/dev/null || true
done
# ELF interpreter symlink: every dynamically-linked binary's PT_INTERP hard-
# codes the SHORT canonical path "/lib/ld-linux-<arch>.so.1" (confirmed via
# `readelf -l /usr/bin/java`), not the multiarch-qualified path the actual
# file lives at -- Debian resolves that short path via a symlink at
# /lib/ld-linux-*.so.1 -> $MULTIARCH/ld-linux-*.so.1, which only exists on
# the REAL filesystem, not automatically in a from-scratch rootfs. Missing
# this made every execve() into this jail fail with a misleading "No such
# file or directory" even though /usr/bin/java itself was right there --
# found empirically by comparing `readelf -l`'s INTERP entry against what
# this rootfs actually contained, not guessed.
# `cp -a` on the symlink itself (not its target, and not `cp -aL`) preserves
# it as a relative symlink ("$MULTIARCH/ld-linux-*.so.1"), which resolves
# correctly relative to its own new location at "$JAVA_ROOT/lib/".
cp -a /lib/ld-linux-*.so.1 "$JAVA_ROOT/lib/" 2>/dev/null || true
# gconv-modules.cache only (not the full ~20MB gconv/ dir of per-encoding .so
# modules): confirmed via strace that the JVM opens/stats this cache file at
# startup, but none of the 13 test-snippets ever caused an actual per-encoding
# converter .so to be dlopen'd (source is fixed at UTF-8 -- javac -encoding
# UTF-8 -- so no exotic charset conversion is exercised). If a user program
# ever forces a legacy charset that needs a real converter module, that would
# surface as a caught UnsupportedEncodingException/IOException in Java (not a
# crash), same "fails safe, not silently" category as the network-isolation
# case documented in JAVA_SECCOMP_POLICY's doc comment -- not verified beyond
# that inference, flagged in tasks.md as a known narrow gap.
cp -a "/usr/lib/$MULTIARCH/gconv/gconv-modules.cache" "$JAVA_ROOT/usr/lib/$MULTIARCH/gconv/" 2>/dev/null || true

# Tiny glibc/NSS config files the JVM startup reads (confirmed via strace, not
# skipped silently) -- SYNTHETIC/minimal versions, NOT copies of the
# container's own /etc/passwd (which lists whatever unrelated system accounts
# happen to exist in the combined API image): only the one uid this sandbox
# actually maps jailed processes to (65534, nobody/nogroup -- see java.rs's
# --uid_mapping/--gid_mapping) needs to resolve. Empty resolver config files
# are enough to satisfy nsswitch/gai/host.conf reads without exposing the real
# ones -- and none of it matters for actual DNS resolution anyway, since
# CLONE_NEWNET (default, see tasks.md "Sem acesso à rede") already means there
# are no network interfaces to resolve anything over.
printf 'nobody:x:65534:65534:nobody:/:/usr/sbin/nologin\n' >"$JAVA_ROOT/etc/passwd"
printf 'nogroup:x:65534:\n' >"$JAVA_ROOT/etc/group"
printf 'hosts: files dns\n' >"$JAVA_ROOT/etc/nsswitch.conf"
: >"$JAVA_ROOT/etc/hosts"
: >"$JAVA_ROOT/etc/resolv.conf"
: >"$JAVA_ROOT/etc/host.conf"
: >"$JAVA_ROOT/etc/gai.conf"
# No /etc/ld.so.cache: deliberately not generated (would need a full
# /etc/ld.so.conf(.d) setup inside this half-formed root for `ldconfig -r` to
# do anything meaningful). glibc's dynamic linker falls back gracefully to its
# compiled-in default search path (/lib/$MULTIARCH, /usr/lib/$MULTIARCH) when
# the cache file is simply absent -- standard, well-documented ld.so behavior,
# not a special case -- and both those directories are exactly where this
# script places every .so above. Validated by actually running the JVM inside
# this rootfs (see tasks.md), not assumed.

# ─── C# rootfs ──────────────────────────────────────────────────────────────
mkdir -p "$CSHARP_ROOT/usr/share/dotnet/host/fxr/$DOTNET_VER" \
         "$CSHARP_ROOT/usr/share/dotnet/shared/Microsoft.NETCore.App/$DOTNET_VER" \
         "$CSHARP_ROOT/usr/share/netcoredbg/netcoredbg" \
         "$CSHARP_ROOT/app" "$CSHARP_ROOT/tmp" "$CSHARP_ROOT/proc" "$CSHARP_ROOT/sys" \
         "$CSHARP_ROOT/dev/shm" "$CSHARP_ROOT/lib/$MULTIARCH" "$CSHARP_ROOT/usr/share/zoneinfo" \
         "$CSHARP_ROOT/workdir"
# /dev/shm: mount point for csharp.rs's --tmpfsmount /dev/shm (see that flag's
# comment). Found empirically, not anticipated: CoreCLR's PAL layer opens
# POSIX named semaphores under /dev/shm/sem.* for the debugger-attach
# handshake specifically (sem.clrco*/sem.clrst* -- "CLR coordination"/"CLR
# startup", confirmed by name and by cross-referencing against the strace
# output) -- a NORMAL (non-debugged) `dotnet <dll>` run doesn't need this at
# all (tested and confirmed working without it), so this was missed on the
# first pass and only surfaced once the real ICorDebug handshake
# (RegisterForRuntimeStartup) was exercised end-to-end, failing with
# hr=0x80070490 (ERROR_NOT_FOUND) with /dev/shm entirely absent.
# /workdir: same fixed mount point as the Java rootfs's identical directory
# -- see that comment above (csharp.rs's JAIL_WORKDIR).

# dotnet HOST + shared RUNTIME only -- deliberately NOT the ~410MB sdk/ tree
# (Roslyn/MSBuild/templates): `dotnet build` (ProcessSandboxRunner, on the API
# side, OUTSIDE this jail -- see csharp.rs's module doc comment) is what needs
# the SDK; the jailed worker only ever execs `dotnet <already-built.dll>`,
# which is the apphost+hostfxr+hostpolicy+coreclr resolution path, confirmed
# by the strace: every /usr/share/dotnet/* path touched falls under host/fxr/
# or shared/Microsoft.NETCore.App/, never sdk/.
cp -a /usr/share/dotnet/dotnet "$CSHARP_ROOT/usr/share/dotnet/dotnet"
cp -a "/usr/share/dotnet/host/fxr/$DOTNET_VER/." "$CSHARP_ROOT/usr/share/dotnet/host/fxr/$DOTNET_VER/"
cp -a "/usr/share/dotnet/shared/Microsoft.NETCore.App/$DOTNET_VER/." "$CSHARP_ROOT/usr/share/dotnet/shared/Microsoft.NETCore.App/$DOTNET_VER/"
cp -a /usr/share/netcoredbg/netcoredbg/libdbgshim.so "$CSHARP_ROOT/usr/share/netcoredbg/netcoredbg/"
# Self re-exec target (see csharp.rs's module doc comment on the
# self-re-exec/--csharp-worker pattern): the jailed process IS this same
# binary, re-invoked with --chroot now pointing here -- it has to be able to
# find itself at this same absolute path post-chroot.
cp -a /app/sandbox-runner "$CSHARP_ROOT/app/sandbox-runner"

# System libs (confirmed via `ldd` on dotnet/libhostfxr.so/libcoreclr.so/
# libclrjit.so/sandbox-runner/libdbgshim.so, union of all five) plus ICU.
# ICU note: NOT observed being dlopen'd by any strace run (13 C# projects
# incl. StringVar, specifically re-checked for globalization/culture-aware
# formatting since none of the official snippets do date/decimal-with-culture
# formatting) -- included anyway, defensively, unlike the gconv case above.
# Reasoning for the asymmetry: gconv modules only matter for exotic charset
# conversion (source is fixed UTF-8, narrow realistic surface); ICU backs
# ordinary-looking .NET code many real submissions could plausibly hit
# (decimal/double ToString(), DateTime formatting, culture-aware string
# comparison) without doing anything exotic, and Dockerfile.api/.ci/Dockerfile
# already carry the identical "libicu72 needed or dotnet build/run breaks"
# finding for the SDK side of this exact image. Costs ~36MB against JDK's own
# ~300MB, not the dominant factor either way.
for lib in libc.so.6 libdl.so.2 libm.so.6 libpthread.so.0 librt.so.1 \
           libstdc++.so.6* libgcc_s.so.1 ld-linux-*.so.1 \
           libicudata.so.7* libicuuc.so.7* libicui18n.so.7*; do
  cp -a "/lib/$MULTIARCH/"$lib "$CSHARP_ROOT/lib/$MULTIARCH/" 2>/dev/null || true
done
# ELF interpreter symlink -- same finding/fix as the Java rootfs above (see
# that comment): PT_INTERP hardcodes the short "/lib/ld-linux-<arch>.so.1"
# path, which only resolves via this symlink on the real filesystem. NOTE:
# the loop above copies the REAL file (it lives under $MULTIARCH/, matching
# where PT_INTERP's target symlink points); this next line copies only the
# top-level symlink itself -- a first cut of this script had the multiarch
# copy MISSING here (only added for the Java rootfs, not this one), leaving
# a dangling symlink that made every C# execve() fail identically to the two
# bugs above -- found the same way, by directly `chroot`-ing into the built
# rootfs and bisecting until the loader itself could be reached.
cp -a /lib/ld-linux-*.so.1 "$CSHARP_ROOT/lib/" 2>/dev/null || true
# Timezone database: same defensive reasoning as ICU above -- TimeZoneInfo
# lookups are ordinary-looking .NET code, not an exotic feature, and none of
# the official snippets exercise it so it wasn't caught by strace either. Java
# needs no equivalent copy: the JDK ships its OWN embedded tzdata inside the
# tree already copied above (confirmed by the total absence of any
# /usr/share/zoneinfo access in the Java strace output, unlike this C# case).
cp -a /usr/share/zoneinfo/. "$CSHARP_ROOT/usr/share/zoneinfo/" 2>/dev/null || true

# /dev/urandom mount POINT only (empty regular file) -- csharp.rs bind-mounts
# the real /dev/urandom onto this path at nsjail invocation time (--bindmount_ro),
# same pattern as the cwd/workdir bind below. Confirmed via strace that CoreCLR
# genuinely opens /dev/urandom directly (crypto RNG seeding, OpenSSL-backed --
# separate from the `getrandom` syscall already in CSHARP_SECCOMP_POLICY,
# which alone was NOT sufficient -- this was found empirically, not assumed).
# Java needs no equivalent: no /dev/urandom or /dev/random open ever appeared
# in ANY Java strace run -- the JDK's default SecureRandom path resolves
# entirely through the `getrandom` syscall already allowlisted, confirmed by
# the absence, not by reading java.security's file-based fallback docs.
touch "$CSHARP_ROOT/dev/urandom"

# ─── Ruby rootfs ────────────────────────────────────────────────────────────
# File list derived EMPIRICALLY, same methodology/tool as the Java/C# lists
# above (strace -e trace=openat,... against `ruby ruby-driver.rb <script>`,
# run OUTSIDE any jail, on a throwaway debian:bookworm-slim container with
# `apt-get install ruby` -- see RUBY_SECCOMP_POLICY's doc comment in
# sandbox/src/ruby.rs for the exact 9 probes this was cross-checked
# against, syscalls and file paths derived from the SAME strace runs).
mkdir -p "$RUBY_ROOT/usr/bin" "$RUBY_ROOT/app" \
         "$RUBY_ROOT/tmp" "$RUBY_ROOT/proc" "$RUBY_ROOT/sys" "$RUBY_ROOT/etc" \
         "$RUBY_ROOT/lib/$MULTIARCH" \
         "$RUBY_ROOT$RUBY_LIBDIR" "$RUBY_ROOT$RUBY_ARCHDIR" \
         "$RUBY_ROOT$RUBY_VENDORLIBDIR" "$RUBY_ROOT$RUBY_VENDORARCHDIR" \
         "$RUBY_ROOT/usr/lib/ruby/gems/$RUBY_ABI_VERSION/specifications/default" \
         "$RUBY_ROOT/workdir"
# /workdir: same fixed bind-mount target as the Java/C# rootfs's identical
# directory -- see those comments above (ruby.rs's JAIL_WORKDIR).

# The `ruby` binary itself (resolved past the update-alternatives symlink,
# same pattern as JDK_HOME above) plus the SAME "ruby" name it's invoked as
# (ruby.rs execs "/usr/bin/ruby", not the versioned binary name directly --
# keeps ruby.rs's hardcoded path stable across a future Ruby version bump
# in the base image, exactly like /usr/bin/java's symlink does for Java).
cp -a "$RUBY_BIN" "$RUBY_ROOT/usr/bin/$(basename "$RUBY_BIN")"
ln -s "/usr/bin/$(basename "$RUBY_BIN")" "$RUBY_ROOT/usr/bin/ruby"

# Pure-Ruby stdlib + compiled C-extension stdlib (json/ext, objspace.so,
# etc.) + vendor dirs (empty on a fresh install today, copied anyway for
# the same "don't special-case an edge that costs nothing to cover"
# reasoning already used for C#'s vendor/site dirs). Whole directories, not
# file-by-file -- same "copy the self-contained tree" reasoning as the JDK
# copy above: driver.rb only directly `require`s 'json'/'objspace', but
# Ruby's own boot sequence and RubyGems' default-gem activation touch a
# much broader set of stdlib files (confirmed via strace) that would be
# tedious and fragile to enumerate by hand.
cp -a "$RUBY_LIBDIR/." "$RUBY_ROOT$RUBY_LIBDIR/"
cp -a "$RUBY_ARCHDIR/." "$RUBY_ROOT$RUBY_ARCHDIR/"
[ -d "$RUBY_VENDORLIBDIR" ] && cp -a "$RUBY_VENDORLIBDIR/." "$RUBY_ROOT$RUBY_VENDORLIBDIR/" 2>/dev/null || true
[ -d "$RUBY_VENDORARCHDIR" ] && cp -a "$RUBY_VENDORARCHDIR/." "$RUBY_ROOT$RUBY_VENDORARCHDIR/" 2>/dev/null || true

# RubyGems default-gem bootstrap: found empirically (NOT anticipated) that
# `require 'json'`/`require 'objspace'` in driver.rb -- even though both
# resolve to plain files already covered by RUBY_LIBDIR/RUBY_ARCHDIR above,
# confirmed via $LOADED_FEATURES -- ALSO makes RubyGems scan
# .../gems/<ruby_version>/specifications/default/*.gemspec on EVERY single
# run (407+ openat calls in every one of the 9 probes, not just the ones
# that explicitly touch RubyGems) to resolve "json"/"objspace" as default
# gems before falling through to the plain $LOAD_PATH lookup. Only the
# (tiny, ~364KB) specifications/default directory is needed -- confirmed
# via strace that NONE of the actual gems/<ruby_version>/gems/<name>/ install
# dirs are ever opened (Debian's packaging keeps default-gem CODE directly
# under rubylibdir/archdir above, the gemspec files are only bookkeeping
# metadata for RubyGems' own activation logic) -- so the much larger
# (~11MB) full gems/ tree (cache/plugins/build_info/doc/actual per-gem
# installs) is deliberately NOT copied, same "only what strace actually
# showed being opened" discipline as everywhere else in this script.
cp -a "/usr/lib/ruby/gems/$RUBY_ABI_VERSION/specifications/default/." \
      "$RUBY_ROOT/usr/lib/ruby/gems/$RUBY_ABI_VERSION/specifications/default/" 2>/dev/null || true

# System libs `ruby`/libruby.so DT_NEED (confirmed via `ldd`, not guessed):
# libruby itself, plus libz (used by zlib.so.2/gem compression internals),
# libgmp (Integer bignum arithmetic -- MRI's Integer overflows into GMP for
# values beyond a machine word, confirmed by ldd even though no probe here
# happened to allocate a bignum), libcrypt (String#crypt/legacy password
# hashing, part of libruby's own link set regardless of use), plus the same
# base glibc pieces the Java rootfs already needs.
# Real bug found and fixed while validating this rootfs for the first time
# (see tasks.md): `libz.so.1`/`libcrypt.so.1` WITHOUT a trailing glob are
# themselves symlinks (-> libz.so.1.2.13 / libcrypt.so.1.1.0 on this base
# image) -- `cp -a` on a bare symlink name preserves just the symlink, not
# its target, so the FIRST version of this loop shipped a dangling symlink
# in the rootfs. `ruby` failed to even start: "error while loading shared
# libraries: libz.so.1: cannot open shared object file" -- confirmed by
# running the real jailed binary, not caught by only inspecting the script.
# `libz.so.1*`/`libcrypt.so.1*` (trailing glob, same trick already used for
# libruby-*.so.*/libgmp.so.10* below) match BOTH the symlink and its real
# versioned target file in one shot.
for lib in libc.so.6 libdl.so.2 libm.so.6 libpthread.so.0 librt.so.1 \
           libruby-*.so.* libz.so.1* libgmp.so.10* libcrypt.so.1* \
           ld-linux-*.so.1; do
  cp -a "/lib/$MULTIARCH/"$lib "$RUBY_ROOT/lib/$MULTIARCH/" 2>/dev/null || true
done
# ELF interpreter symlink -- same finding/fix as the Java/C# rootfs above
# (PT_INTERP hardcodes the short "/lib/ld-linux-<arch>.so.1" path, which
# only resolves via this symlink on the real filesystem). Confirmed via
# `readelf -l` on the real ruby3.1 binary inside the probe container:
# "Requesting program interpreter: /lib/ld-linux-aarch64.so.1", same as
# every other dynamically-linked ELF binary this script handles.
cp -a /lib/ld-linux-*.so.1 "$RUBY_ROOT/lib/" 2>/dev/null || true

# ruby-driver.rb: the driver itself (sandbox/ruby/driver.rb), staged by the
# Dockerfile at /app/ruby-driver.rb -- no build/compile step needed (unlike
# jdi-driver.jar/.jsa above), it's copied here as plain source, same fixed
# path ruby.rs::RUBY_DRIVER_PATH hardcodes.
cp -a /app/ruby-driver.rb "$RUBY_ROOT/app/ruby-driver.rb"

# Same synthetic, minimal glibc/NSS config files as the Java rootfs above --
# see that block's comment for the full rationale (only uid 65534 needs to
# resolve; DNS resolution doesn't matter either way since CLONE_NEWNET
# already means there's nothing to resolve over).
printf 'nobody:x:65534:65534:nobody:/:/usr/sbin/nologin\n' >"$RUBY_ROOT/etc/passwd"
printf 'nogroup:x:65534:\n' >"$RUBY_ROOT/etc/group"
printf 'hosts: files dns\n' >"$RUBY_ROOT/etc/nsswitch.conf"
: >"$RUBY_ROOT/etc/hosts"
: >"$RUBY_ROOT/etc/resolv.conf"
: >"$RUBY_ROOT/etc/host.conf"
: >"$RUBY_ROOT/etc/gai.conf"

echo "[minimal-rootfs] done: $(du -sh "$JAVA_ROOT" | cut -f1) java, $(du -sh "$CSHARP_ROOT" | cut -f1) csharp, $(du -sh "$RUBY_ROOT" | cut -f1) ruby"
