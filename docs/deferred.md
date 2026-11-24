# Deferred work

Parked, not blocked. Nothing here is next until the terminal itself is
solid and the project is otherwise quiet.

## On hold until muis is 100% sorted

- **Snor engine.** The side-panel slot API is already in place
  (`SidePanelRegistry`: register/toggle/single-visible/unregister, see
  `docs/archive/build-progress.md` Step 6). No panel code exists yet.
  Plan: `docs/snor-plan.md`. Revisit once the terminal is done.

## Removed build paths

Windows (portable exe and installer), Flatpak, AppImage, and the AUR package
were removed to keep the build and release path focused on the Linux artifact
Veldmuis repackages (`muis-<version>-x86_64-linux.tar.gz`). Their CI jobs,
packaging files, docs, and platform-specific code are gone.

Windows is now supported from source instead, through the browser server
(`cargo run -p muis-server`, see the README): the same crates compile for
`x86_64-pc-windows-gnu`, CI cross-checks them, and there is deliberately no
installer, no WebView2 dependency, and no packaged `.exe` to hand around.
A packaged Windows artifact stays deferred.

Restore from git history if support is revisited: start with the tree before
this removal, especially `.github/workflows/release.yml`, `packaging/flatpak/`,
`aur/`, `crates/muis-shell/tauri.conf.json`, and the platform branches in
`crates/muis-worker`.

## Settled scope

- **Updates:** package managers only. muis makes no update checks and never
  self-updates; the website links to GitHub releases.
- **Website:** the landing page lives in `ruannnebornman/muis-website` and is
  deployed to `muis.veldmuislinux.org` through Cloudflare Pages.
- **Packaging:** the single Linux tarball. Windows, Flatpak, AppImage, and AUR
  stay removed.
- **aarch64:** not planned; Veldmuis targets x86_64.

## Abandoned

- **Veldmuis default terminal** (rollout Phase 4). Veldmuis ships Konsole
  as its default; muis is an optional `veldmuis-muis` package. The old
  `veldmuis-kaazrot`/wezterm default-terminal wiring is gone.
- **Export/import of session state.** It existed for the HTML preview
  mock only; not wanted.
- **Split panes.** Not wanted; muis is one terminal per tab.
