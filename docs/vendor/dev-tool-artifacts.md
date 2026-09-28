<!-- derived from: web research (tool docs, source repos) @ 2026-09-28 -->

# Developer-tool cleanup artifact catalog (macOS)

Per-tool inventory of cache/build artifacts a cleanup CLI ("rosie") could target, with
detection markers, regenerability, and whether the tool's own cleanup command should be
preferred over raw `rm -rf`. Paths use `~` for `$HOME`.

## Xcode

| artifact              | path                                                              | detection marker                              | regenerable? | prefer native cleanup            |
|------------------------|--------------------------------------------------------------------|------------------------------------------------|---------------|------------------------------------|
| DerivedData            | `~/Library/Developer/Xcode/DerivedData/<Project-hash>/`            | folder name suffixed with a hash; sibling has no manifest needed, whole dir is cache | yes | delete after quitting Xcode; no dedicated CLI, `xcodebuild clean` only cleans one target |
| Archives (.xcarchive)  | `~/Library/Developer/Xcode/Archives/<date>/`                       | `.xcarchive` bundle                             | **no** — contains dSYMs needed to symbolicate crashes for shipped builds | do not auto-delete; only prune by age/user confirmation |
| iOS/watchOS DeviceSupport | `~/Library/Developer/Xcode/iOS DeviceSupport/`, `.../watchOS DeviceSupport/`, `tvOS DeviceSupport/` | dir named `<OS> DeviceSupport`, one subfolder per device+OS version | yes — re-downloaded on next device connect for that OS version | no CLI; safe manual delete of old-version subfolders |
| Simulator devices/data | `~/Library/Developer/CoreSimulator/Devices/`                       | UUID-named subfolders; unavailable ones point at missing runtimes | yes | prefer `xcrun simctl delete unavailable` (and `xcrun simctl erase all` for data) over raw deletion |
| Simulator caches       | `~/Library/Developer/CoreSimulator/Caches/`                        | cache dir under CoreSimulator                   | yes | safe manual delete |
| Xcode Previews cache   | `~/Library/Developer/Xcode/UserData/Previews/` and `DerivedData/.../Build/Products/*-Simulator` preview artifacts | subpath of DerivedData or UserData | yes | delete with DerivedData |
| SwiftPM cache (Xcode-integrated) | `~/Library/Caches/org.swift.swiftpm/` (see SwiftPM section) | | yes | see SwiftPM |

Detection note: none of Xcode's caches need a sibling-project marker — they all live
under the fixed `~/Library/Developer/Xcode` tree, keyed by hash/UUID, not by scanning
project directories.

Source: [dev.to Xcode cleanup guide](https://dev.to/pyrx_tech/the-complete-xcode-cleanup-guide-for-iosmacos-developers-reclaim-50-150-gb-2026-2h5j),
[cleanwithcrumb ~/Library/Developer guide](https://cleanwithcrumb.com/blog/empty-library-developer-folder-mac/) — read 2026-09-28.

## Docker Desktop / container runtimes

| artifact                   | path                                                                 | detection marker                     | regenerable? | prefer native cleanup |
|------------------------------|-------------------------------------------------------------------------|-----------------------------------------|---------------|--------------------------|
| Docker Desktop VM disk image | `~/Library/Containers/com.docker.docker/Data/vms/0/Docker.raw`         | fixed path, presence of `com.docker.docker` container | contains all images/volumes/containers — **not directly deletable safely** | strongly prefer `docker system prune` (+`-a --volumes` for aggressive) or Docker Desktop's own Settings → Resources → "Disk image" resize/reset |
| Images, containers, build cache, networks | managed inside the VM, reported via `docker system df`     | n/a — must query via Docker API/CLI, not filesystem scan | dangling images/build cache: yes; tagged images/volumes: no (data loss) | always `docker system df` to see RECLAIMABLE, then `docker system prune` |
| OrbStack data               | `~/Library/Group Containers/HUAQ24HBR6.dev.orbstack/data`              | fixed container-group path              | same caveat as Docker.raw | prefer `orbctl run docker system prune -a --volumes -f` then `orbctl compact` |
| Colima/Lima VM disks        | `~/.colima/<profile>/`, `~/.lima/<instance>/diffdisk`                  | profile dir under `.colima`/`.lima`; stale instances left after `colima delete` | deleting a stopped profile's dir is safe, but destroys that VM's containers/images | prefer `colima delete <profile>` / `colima prune` over raw deletion of `.lima` dirs |

Detection note: raw filesystem deletion of the VM disk file is unsafe/destructive by
design (it's a single opaque disk image holding all container state); rosie should treat
Docker/OrbStack/Colima as "shell out to the tool's own prune/compact," not a
path-based rule, except for identifying stale/orphaned `.lima` instance dirs.

Source: [Dash0 Docker disk cleanup](https://www.dash0.com/faq/how-to-clean-up-docker-disk-space),
[OrbStack FAQ](https://orbstack.dev/docs/faq),
[CleanMyDev Colima/OrbStack disk usage](https://cleanmydev.com/blog/colima-orbstack-disk-usage/) —
read 2026-09-28.

## Homebrew

| artifact          | path                                          | detection marker            | regenerable? | prefer native cleanup |
|--------------------|-------------------------------------------------|--------------------------------|---------------|--------------------------|
| Download cache     | `$(brew --cache)` (typically `~/Library/Caches/Homebrew`) | fixed, query via `brew --cache` (differs Intel vs Apple Silicon prefix) | yes | prefer `brew cleanup` (removes old Cellar versions + cache); `brew cleanup` alone historically leaves `~/Library/Caches/Homebrew` fully clean only with `-s`/prune-all flags |
| Old Cellar versions| `/opt/homebrew/Cellar/<formula>/<old-version>` or `/usr/local/Cellar/...` | superseded version dirs after upgrade | yes | `brew cleanup`, not manual `rm` (breaks brew's DB) |

Source: [Homebrew Cleanup docs](https://docs.brew.sh/rubydoc/Homebrew/Cleanup.html),
[Homebrew/brew#3784 cache cleanup issue](https://github.com/Homebrew/brew/issues/3784) — read
2026-09-28.

## npm / pnpm / yarn / bun / node_modules

| artifact              | path                                                          | detection marker                                  | regenerable? | prefer native cleanup |
|------------------------|------------------------------------------------------------------|------------------------------------------------------|---------------|--------------------------|
| npm cache              | `~/.npm`                                                        | fixed global path                                     | yes | `npm cache clean --force` (raw delete works too, npm rebuilds) |
| yarn classic cache     | `~/Library/Caches/Yarn`                                         | fixed global path                                     | yes | `yarn cache clean` |
| yarn berry (v2+) cache | `~/.yarn/berry/cache`                                           | fixed global path; project may use zero-installs (`.yarn/cache` checked in — do NOT delete blindly) | yes, unless zero-install | `yarn cache clean`; check for `.yarnrc.yml` `nodeLinker`/`enableGlobalCache` before touching project-local `.yarn/cache` |
| pnpm content-addressable store | `~/.pnpm-store` or `~/.local/share/pnpm/store`         | fixed global path                                     | yes | prefer `pnpm store prune` (removes only unreferenced packages) over raw delete |
| bun install cache      | `~/.bun/install/cache`                                          | fixed global path                                     | yes | no dedicated prune subcommand found; safe raw delete |
| `node_modules/`        | `<project>/node_modules/`                                       | sibling `package.json` (and optionally lockfile: `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `bun.lockb`) in same dir | yes, reinstall from lockfile | no universal "clean" command; raw delete is standard practice (`rm -rf node_modules`) |

Detection note: `node_modules` is the classic "per-project regenerable tree" case —
always require a sibling manifest (`package.json`) before treating a `node_modules` dir
as safe, since a stray copied folder without a manifest can't be regenerated.

Source: [pnpm store CLI docs](https://pnpm.io/cli/store),
[bysiber npm/yarn/pnpm cache guide](https://bysiber.github.io/cleardisk/blog/npm-yarn-pnpm-cache-cleanup-mac.html) —
read 2026-09-28.

## Cargo (Rust)

| artifact                 | path                              | detection marker                                                    | regenerable? | prefer native cleanup |
|----------------------------|--------------------------------------|--------------------------------------------------------------------------|---------------|--------------------------|
| `target/` build dir        | `<crate>/target/`                    | sibling `Cargo.toml` in same dir, **and/or** `target/CACHEDIR.TAG` or `target/.rustc_info.json` present inside it | yes | `cargo clean` (whole target) or `cargo-sweep` (age/keep-N-based partial clean) — prefer sweep for active repos |
| Registry cache (index/cache/src) | `~/.cargo/registry/{index,cache,src}` | fixed global path under `$CARGO_HOME` (default `~/.cargo`)          | yes | `cargo-cache` crate provides targeted commands; raw delete works but re-download is slow |
| Git checkouts/db           | `~/.cargo/git/{db,checkouts}`        | fixed global path under `$CARGO_HOME`                                    | yes | same as above |

Detection note: `cargo clean` only touches the project's own `target/`, never the global
`~/.cargo` caches — rosie needs both a per-project rule (sibling `Cargo.toml`) and a
separate global-cache rule under `$CARGO_HOME`. As of this research, cargo itself does
not yet tag `target/` with `CACHEDIR.TAG` by default (tracked in
rust-lang/cargo#10457), so `target/.rustc_info.json` is the more reliable marker; sibling
`Cargo.toml` is the primary signal either way.

Source: [rust-lang/cargo#10457](https://github.com/rust-lang/cargo/issues/10457),
[matthiaskrgr/cargo-cache](https://github.com/matthiaskrgr/cargo-cache),
[cargo-sweep](https://dev.to/nixeton/cargo-target-directories-why-rust-projects-eat-10gb-each-26kd) —
read 2026-09-28.

## Go

| artifact          | path                                   | detection marker                     | regenerable? | prefer native cleanup |
|---------------------|--------------------------------------------|------------------------------------------|---------------|--------------------------|
| Build cache (GOCACHE) | `~/Library/Caches/go-build` (default; `go env GOCACHE` to confirm) | fixed path, override via `$GOCACHE`  | yes | `go clean -cache` |
| Module cache (GOMODCACHE) | `~/go/pkg/mod` (default; `go env GOMODCACHE`)                  | fixed path, override via `$GOMODCACHE` | yes | `go clean -modcache` (raw delete leaves read-only perms that error on plain `rm -rf` — use `chmod -R +w` first or the go command) |
| Test cache          | inside GOCACHE                             | part of GOCACHE                          | yes | `go clean -testcache` |

Detection note: always resolve paths via `go env GOCACHE`/`go env GOMODCACHE` rather than
hardcoding, since users commonly override both via env vars.

Source: [educative.io go clean -modcache](https://www.educative.io/answers/what-is-go-clean--modcache),
[golang/go#31283](https://github.com/golang/go/issues/31283) — read 2026-09-28.

## Gradle / Maven / .m2

| artifact              | path                                                     | detection marker                    | regenerable? | prefer native cleanup |
|------------------------|--------------------------------------------------------------|-----------------------------------------|---------------|--------------------------|
| Gradle dependency cache | `~/.gradle/caches/modules-2/files-2.1`                       | fixed path under `~/.gradle`            | yes | no built-in prune subcommand; `gradle --stop` first to kill daemons holding file locks, then raw delete of `~/.gradle/caches` is the documented approach |
| Gradle wrapper distributions | `~/.gradle/wrapper/dists`                                | fixed path                              | yes, re-downloaded per `gradle-wrapper.properties` | raw delete safe |
| Gradle project build dirs | `<project>/**/build/`                                      | sibling `build.gradle`/`build.gradle.kts`/`settings.gradle` | yes | `./gradlew clean` preferred (respects per-task outputs) |
| Maven local repo      | `~/.m2/repository`                                            | fixed path, structured `groupId/artifactId/version` tree | yes, unless it holds locally `mvn install`-ed artifacts not in any remote repo | `mvn dependency:purge-local-repository` for a targeted purge; raw `rm -rf` acceptable if no local-only installs |

Source: [codestudy.net Gradle vs Maven storage](https://www.codestudy.net/blog/where-does-gradle-store-downloaded-jars-on-the-local-file-system/),
[Baeldung Maven cache](https://www.baeldung.com/maven-clear-cache) — read 2026-09-28.

## CocoaPods

| artifact          | path                                | detection marker                          | regenerable? | prefer native cleanup |
|---------------------|-----------------------------------------|-----------------------------------------------|---------------|--------------------------|
| Global pod cache    | `~/Library/Caches/CocoaPods`            | fixed global path                              | yes | `pod cache clean --all` |
| Specs repo mirror   | `~/.cocoapods/repos`                    | fixed global path                              | yes (re-clonable) | `pod repo remove <name>` / `pod setup` |
| Project `Pods/`     | `<project>/Pods/`                       | sibling `Podfile.lock` in project root         | yes, reinstall via `pod install` | raw delete acceptable but keep `Podfile.lock` — deleting it changes resolved versions |

Source: [CocoaPods FAQ](https://guides.cocoapods.org/using/faq.html) — read 2026-09-28.

## Swift Package Manager (SwiftPM)

| artifact                | path                                             | detection marker                    | regenerable? | prefer native cleanup |
|---------------------------|-------------------------------------------------------|------------------------------------------|---------------|--------------------------|
| Shared SwiftPM cache      | `~/Library/Caches/org.swift.swiftpm/` (repositories/, artifacts, registry responses) | fixed global path | yes | `swift package purge-cache` |
| Project `.build/`         | `<package>/.build/`                                    | sibling `Package.swift`                  | yes | `swift package clean` (or raw delete) |

Source: [uptech SwiftPM caching](https://www.uptech.team/blog/swift-package-manager),
[eidinger.info purge-cache](https://blog.eidinger.info/swift-package-purge-cache) — read 2026-09-28.

## Python: pip / uv / poetry / conda / .venv

| artifact         | path                                       | detection marker                | regenerable? | prefer native cleanup |
|--------------------|-------------------------------------------------|-------------------------------------|---------------|--------------------------|
| pip cache          | `~/Library/Caches/pip`                          | fixed global path                    | yes | `pip cache purge` |
| uv cache           | `~/.cache/uv`                                   | fixed global path (1-5 GB typical)   | yes | `uv cache clean` |
| poetry cache       | `~/Library/Caches/pypoetry`                     | fixed global path                    | yes | `poetry cache clear --all` (or per-cache-name) |
| conda pkg cache    | `~/opt/anaconda3/pkgs` or `~/miniconda3/pkgs` (env-manager-specific, use `conda info --base`) | fixed under conda base install | yes | `conda clean --all` |
| `__pycache__/`     | anywhere under a Python project (`<dir>/__pycache__/`) | dir name is the marker itself; contains only `.pyc` | yes | `find . -name __pycache__ -exec rm -rf {} +` or `pyclean`; no single canonical CLI |
| `.venv` / `venv`   | `<project>/.venv/` or `<project>/venv/`         | sibling `pyproject.toml`/`requirements.txt`, and presence of `pyvenv.cfg` inside the dir confirms it's a real venv (not user data) | yes, reinstall from lock/requirements | raw delete standard; no native "clean" subcommand (venvs are just directories) |

Detection note: `pyvenv.cfg` inside the candidate dir is the reliable marker that a
`.venv`-named folder really is a virtualenv, not an unrelated user folder someone named
`.venv`.

Source: [bysiber Python cache guide](https://bysiber.github.io/cleardisk/blog/python-pip-cache-cleanup-mac.html),
[deepclean Python cache guide](https://deepclean.app/blog/clean-python-cache-mac) — read 2026-09-28.

## Ruby gems / Bundler

| artifact              | path                                          | detection marker           | regenerable? | prefer native cleanup |
|------------------------|--------------------------------------------------|--------------------------------|---------------|--------------------------|
| RubyGems cache/install | `$(gem environment gemdir)/cache`, `.../gems`, `.../doc` | fixed path (varies with rbenv/rvm/system Ruby — must shell out to `gem environment`) | yes | `gem cleanup` removes unused old versions + their cached `.gem` files |
| Bundler user cache     | `~/.bundle/cache` (overridable via `$BUNDLE_USER_CACHE`) | fixed global path              | yes | no dedicated purge; raw delete safe |
| Project vendored gems  | `<project>/vendor/bundle/`                        | sibling `Gemfile.lock`          | yes, reinstall via `bundle install` | `bundle clean` removes gems no longer in `Gemfile.lock` (leaves cached `.gem` files behind per rubygems/rubygems#7163) |

Source: [RubyGems caching guide](https://guides.rubygems.org/caching-and-vendoring/),
[ruby/rubygems#7163](https://github.com/ruby/rubygems/issues/7163) — read 2026-09-28.

## JetBrains IDEs

| artifact       | path                                                       | detection marker                          | regenerable? | prefer native cleanup |
|------------------|-----------------------------------------------------------------|-----------------------------------------------|---------------|--------------------------|
| Caches           | `~/Library/Caches/JetBrains/<Product><Version>/`                 | versioned product subfolder under fixed parent | mostly, re-indexes on next launch — but also holds `LocalHistory/`, the IDE's record of uncommitted edits, which is not regenerable | no CLI; raw delete safe for the cache itself, but IDE should be closed and Local History is lost |
| Logs             | `~/Library/Logs/JetBrains/<Product><Version>/`                   | same pattern                                   | yes | raw delete safe |
| Plugins/settings | `~/Library/Application Support/JetBrains/<Product><Version>/`    | same pattern — **not purely cache**, contains settings/plugins | no (user config) | exclude from cleanup rules by default |

Source: [JetBrains directories doc](https://www.jetbrains.com/help/idea/directories-used-by-the-ide-to-store-settings-caches-plugins-and-logs.html) — read 2026-09-28.

## VS Code

| artifact                   | path                                              | detection marker         | regenerable? | prefer native cleanup |
|------------------------------|--------------------------------------------------------|-------------------------------|---------------|--------------------------|
| Cache / CachedData / logs    | `~/Library/Application Support/Code/Cache`, `CachedData`, `logs` | fixed subpaths under `Code/` | yes | no CLI; raw delete safe, rebuilt on launch |
| CachedExtensionVSIXs         | `~/Library/Application Support/Code/CachedExtensionVSIXs` | fixed subpath              | yes | raw delete safe |
| Extensions dir (not cache)   | `~/.vscode/extensions`                                  | fixed path — **not a cache**, deleting removes installed extensions | no | exclude by default |

Source: [bysiber IDE cache guide](https://bysiber.github.io/cleardisk/blog/ide-cache-cleanup-mac.html) — read 2026-09-28.

## Android SDK / emulators / AVDs

| artifact                | path                                         | detection marker                | regenerable? | prefer native cleanup |
|---------------------------|---------------------------------------------------|--------------------------------------|---------------|--------------------------|
| SDK packages (platforms, build-tools, system images) | `~/Library/Android/sdk/`         | fixed path (or `$ANDROID_HOME`/`$ANDROID_SDK_ROOT`) | yes, re-downloadable via `sdkmanager` | prefer `sdkmanager --uninstall <package>` over raw delete to keep SDK manifest consistent |
| AVD definitions + userdata images | `~/.android/avd/<name>.avd/`, config at `~/.android/avd/<name>.ini` | `.avd` dir + matching `.ini` file pair | data images regenerable by recreating AVD; **user data/snapshots inside are not** | prefer `avdmanager delete avd -n <name>` over raw `rm -rf` of the `.avd` dir (keeps the `.ini` index in sync) |
| Gradle build cache for Android projects | see Gradle section | sibling `build.gradle(.kts)` | yes | `./gradlew clean` |

Detection note: an orphaned `.ini` file without its `.avd` dir (or vice versa) indicates
a broken/stale AVD entry safe to clean via `avdmanager`, not by hand.

Source: [Android Studio AVD docs](https://developer.android.com/studio/run/managing-avds) —
read 2026-09-28.

## Flutter / Dart

| artifact               | path                                | detection marker                    | regenerable? | prefer native cleanup |
|--------------------------|------------------------------------------|------------------------------------------|---------------|--------------------------|
| Pub package cache        | `~/.pub-cache/`                          | fixed global path (override via `$PUB_CACHE`) | yes | `dart pub cache clean` (Dart ≥2.14); manual delete on older SDKs |
| Project build output     | `<project>/build/`                       | sibling `pubspec.yaml`                   | yes | `flutter clean` |
| Project `.dart_tool/`    | `<project>/.dart_tool/`                  | sibling `pubspec.yaml`                   | yes | `flutter clean` (removes both `build/` and `.dart_tool/`) |

Source: [Dart pub cache docs](https://dart.dev/tools/pub/cmd/pub-cache) — read 2026-09-28.

## JS/TS framework build outputs

| artifact       | path                    | detection marker                                      | regenerable? | prefer native cleanup |
|------------------|------------------------------|------------------------------------------------------------|---------------|--------------------------|
| Next.js `.next/`  | `<project>/.next/`           | sibling `next.config.js`/`.mjs`/`.ts` or `package.json` with `next` dep | yes | no dedicated CLI clean beyond `rm -rf .next`; raw delete standard |
| Nuxt `.nuxt/`     | `<project>/.nuxt/`           | sibling `nuxt.config.ts`/`.js`                              | yes | `nuxi cleanup` (Nuxt 3) removes `.nuxt`, `.output`, `node_modules/.cache`, `node_modules/.vite` |
| Generic `dist/`   | `<project>/dist/`            | sibling `package.json` with a build script; ambiguous alone since many tools use `dist` — require sibling manifest, not just dir name | yes | tool-specific build script re-run (`npm run build`), no universal clean |

Detection note: `dist/` is the riskiest generic name in this catalog — never match on
name alone; require a sibling `package.json` (or equivalent) plus, ideally, that the dir
is listed in a `.gitignore` before treating it as disposable build output.

## Cargo/Rust and general cache-dir convention

| convention        | meaning                                                       |
|----------------------|--------------------------------------------------------------|
| `CACHEDIR.TAG` file  | [Cache Directory Tagging Specification](https://bford.info/cachedir/) — a directory containing a file literally named `CACHEDIR.TAG` starting with `Signature: 8a477f597d28d172789f06886806bc55` is self-declared as safe-to-delete cache. Tools like `ncdu`, `restic`, backup software honor this. Rosie could both *read* this marker for third-party tools that emit it and *emit* it on its own scratch dirs. |

## Terraform

| artifact            | path                                  | detection marker                     | regenerable? | prefer native cleanup |
|-----------------------|--------------------------------------------|-------------------------------------------|---------------|--------------------------|
| `.terraform/` working dir | `<module>/.terraform/`                 | sibling `.tf` files, presence of `.terraform.lock.hcl` | yes, `terraform init` re-populates | raw delete acceptable; re-run `terraform init` |
| Provider plugin cache | `~/.terraform.d/plugin-cache` (if configured via `plugin_cache_dir` in `~/.terraformrc` or `$TF_PLUGIN_CACHE_DIR`) | path is user-configured, not fixed — must read `~/.terraformrc` or env var to locate | yes | raw delete safe, slows next `init` across all projects using the cache |

Source: [makandra Terraform plugin cache](https://makandracards.com/operations/510564-terraform-plugin-cache) —
read 2026-09-28.

## iOS device backups

| artifact           | path                                                         | detection marker              | regenerable? | prefer native cleanup |
|-----------------------|-------------------------------------------------------------------|------------------------------------|---------------|--------------------------|
| iTunes/Finder device backups | `~/Library/Application Support/MobileSync/Backup/<UDID>/`   | UUID-named subfolder under fixed `MobileSync/Backup` | **no** — these are user backups, not caches; deleting loses the only local copy of a device's data | never auto-delete; only list by age/size for user review, or point users at Finder's own "Manage Backups" UI |

## System / user logs

| artifact             | path                                                    | detection marker           | regenerable? | prefer native cleanup |
|------------------------|--------------------------------------------------------------|---------------------------------|---------------|--------------------------|
| User app logs          | `~/Library/Logs/<vendor>/...` (per-tool subfolders, e.g. `~/Library/Logs/JetBrains`, `~/Library/Logs/Homebrew`) | vendor-named subfolder under fixed `~/Library/Logs` | yes, logs regenerate on next run | generally safe to age-prune (e.g. delete files older than N days); no single native command covers all vendors |
| System diagnostic logs | `/Library/Logs/DiagnosticReports/`, `~/Library/Logs/DiagnosticReports/` | fixed path, crash/hang report files | yes (historical debugging value only) | safe to prune by age; macOS itself rotates/expires some automatically |

---

## Summary: strongly prefer native cleanup over raw deletion

- **Docker/OrbStack/Colima**: raw deletion of the VM disk is destructive/unsafe by
  design — always shell out to `docker system prune`, `orbctl compact`, or
  `colima prune`/`colima delete`.
- **Homebrew**: use `brew cleanup`, not manual `rm`, since Homebrew tracks installed
  versions in its own DB that raw deletion desyncs.
- **pnpm**: `pnpm store prune` (content-addressable store shared across projects; naive
  deletion is fine too but the command safely handles cross-project references).
- **Go**: `go clean -cache -modcache` (module cache dirs are made read-only; a plain
  `rm -rf` fails without a preceding `chmod`).
- **Android AVDs/SDK packages**: `avdmanager delete avd` / `sdkmanager --uninstall` keep
  Android's own manifests in sync; raw deletion of `.avd` dirs leaves stale index entries.
- **CocoaPods / SwiftPM / Poetry / uv / conda / gem**: each has a dedicated
  cache-clean/purge subcommand; prefer it since these caches back multiple projects
  simultaneously.
- **Xcode Archives and iOS device backups**: never auto-delete regardless of tool
  availability — these hold irreplaceable data (dSYMs for shipped builds; the only local
  device backup), not regenerable caches.
