# wsl-disk-cleaner

Your C drive is almost full, and you have no idea why. Explorer shows nothing useful. WSL hides everything inside one giant disk file that only grows.

You are not alone. Build tools fill disks quietly. Old build folders, package caches and forgotten test containers pile up for months before anyone looks.

`wdc` answers two questions. Who fills my disk? What can I delete without fear?

## Cleaning by hand is slow and risky

Finding the big folders takes an hour of `du` and PowerShell. Deleting them takes one wrong `rm -rf` to lose source code or work you cannot get back.

`wdc` deletes only build output and caches. Each tool cleans its own files, so `cargo clean` empties a Rust target folder and `podman volume prune` drops unused volumes. You see the full plan before anything happens, and nothing runs until you press `y`. `wdc` never deletes anything on Windows.

## Install with cargo

You need WSL2 and a Rust toolchain.

```sh
git clone https://github.com/alexandremahdhaoui/wsl-disk-cleaner.git
cd wsl-disk-cleaner
cargo build --release
cp target/release/wdc ~/.local/bin/
```

## Scan to see who fills each disk

```sh
wdc scan
```

One tab covers WSL and one covers the Windows C drive. A colored strip splits the disk by its five biggest folders. Below the strip, a tree opens on the folders that matter, so the biggest folders show up without any digging. The Windows tab also shows how big the WSL disk file is and how much WSL really uses inside it.

Tab switches disks. Enter opens or closes a folder. `q` quits.

## Clean with a plan you approve

```sh
wdc clean
```

`wdc` lists what it can free, grouped by tool, with the biggest group first. The strip at the top grows as you check items, so you see the effect before you commit.

Space checks one item. `a` checks all and `n` clears all. Enter shows the exact commands. `y` runs them.

`wdc clean --dry-run` prints the plan and deletes nothing.

| Tool | What `wdc` runs |
|---|---|
| Cargo | `cargo clean` on every `target` folder under your home that Cargo marked with `CACHEDIR.TAG` |
| Go | `go clean -cache -testcache` and `go clean -modcache` |
| uv, pnpm, npm | `uv cache clean`, `pnpm store prune`, `npm cache clean --force` |
| Podman, Docker | `container prune`, `volume prune` and `image prune -a` |
| nerdctl | `nerdctl system prune --all --force --volumes` |
| Buildah | `buildah rm --all` |

Tools you have not installed stay hidden.

## A guard refuses to delete your work

The guard checks every item twice, once when it builds the plan and once right before it deletes. A refused item stays visible in the plan with the reason, and nothing ever deletes it.

The guard refuses a path that:

- sits outside the WSL disk, such as anything under `/mnt/c`
- holds a file tracked by git
- sits inside `~/.claude` or contains it
- looks like a cargo `target` folder but has no `CACHEDIR.TAG` or no `Cargo.toml` next to it

## Compact the WSL disk to give the space back to Windows

WSL keeps freed space inside its disk file. Windows gets it back only after you compact that file. `wdc scan` prints the exact path. Run this in an admin PowerShell:

```powershell
wsl --shutdown
Optimize-VHD -Path '<path from wdc scan>' -Mode Full
```

`Optimize-VHD` needs the Hyper-V PowerShell module.

## Develop with forge

```sh
forge build wdc
forge test-all
```

Apache 2.0 license.
