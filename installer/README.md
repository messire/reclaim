# Installer

Reclaim ships two ways, built from the same binaries:

- **Installer** `reclaim-<version>-windows-x64-setup.exe`: per-user install, Start menu entry,
  uninstaller in Settings → Apps.
- **Portable zip** `reclaim-<version>-windows-x64.zip`: unpack anywhere and run.

## Build it

On Windows, with Rust installed ([rustup](https://rustup.rs) with the MSVC build tools, as
for any build of Reclaim), in the folder you cloned the repository into (the one with
`Cargo.toml`), for example:

```cmd
git clone https://github.com/messire/reclaim
cd reclaim
scripts\build-installer.cmd -Run
```

Or double-click `scripts\build-installer.cmd` in Explorer: the script finds the repository
folder itself, and the window stays open at the end. The results land in `dist\` inside the
repository folder.

The script builds `reclaim.exe` and `reclaim-cli.exe` in release mode, writes
`THIRD_PARTY_NOTICES.txt`, compiles `installer/reclaim.iss` and packs the zip. Both land in
`dist\`. If Inno Setup 6 or Python 3 is missing it installs them with `winget`.

Options (PowerShell switches, passed through by the `.cmd`):

| Option | Effect |
|---|---|
| `-Run` | Start the installer when it is built. |
| `-SkipBuild` | Reuse `target\release` instead of running cargo. |
| `-NoInstallTools` | Fail instead of installing missing tools. |
| `-OutDir <dir>` | Output folder (default `dist`). |

## What the installer does

- **Folder:** the user picks it. The default is `%LOCALAPPDATA%\Programs\Reclaim`, which needs
  no administrator rights. A folder that needs them (such as Program Files) is not supported.
- **Start with Windows:** a checkbox (off by default) writes
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Reclaim = "<folder>\reclaim.exe" --tray`
  during installation. The app's *Settings → Start Reclaim with Windows* writes the same value,
  so the two always agree, and the installer shows the current state when it runs again.
- **Upgrades replace the installed version.** Every version has the same `AppId`
  (`{09543C3B-C0B8-43FA-BC08-E11A6FB50CD4}`, never change it), so a newer setup finds the
  installed one, closes a running Reclaim, and installs over it. The folder is pre-filled with
  the current one; if the user picks another folder, the old copy is uninstalled first. There is
  only ever one Reclaim in Settings → Apps.
- **Uninstall** closes Reclaim and removes the files, the shortcuts and the start-with-Windows
  value. Settings and history in `%LOCALAPPDATA%\Reclaim` are kept; delete that folder (or use
  *Settings → Clear history and logs* first) to remove them.

Silent install for scripts: `setup.exe /VERYSILENT /DIR="C:\Tools\Reclaim" /TASKS=autostart`
(`/MERGETASKS=!autostart` to turn start with Windows off).

CI runs `scripts/test-installer.ps1` on every pull request: install, upgrade in place, upgrade
into another folder while Reclaim runs, uninstall, checking the files, the `Run` value and that
exactly one installation is registered after each step.

## Why Inno Setup

- Per-user installs without administrator rights are a first-class mode
  (`PrivilegesRequired=lowest`), with a folder page and checkbox tasks out of the box.
- Upgrades by a fixed `AppId` replace the installation in place, and closing the running app
  (Restart Manager) is built in.
- One readable script, a free tool, preinstalled on GitHub's Windows runners and installable
  with `winget install JRSoftware.InnoSetup`.

WiX (MSI) was the alternative. Per-user MSI packages are awkward (`ALLUSERS=2` and
`MSIINSTALLPERUSER` quirks), a custom install folder needs extra UI authoring, and major
upgrades need a version scheme MSI accepts; none of it buys Reclaim anything over Inno Setup.
