# Memory baseline

How much memory the shell uses, split by kind, and how to measure it again (#198). Take a new baseline before and after any memory optimization, on the same machine and monitor setup.

## Where the memory is

- **RSS, PSS, anonymous** (`/proc/<pid>/smaps_rollup`): the shell's own pages. Anonymous is mostly heap. Mapped files are shared libraries and fonts.
- **GPU buffers**: wgpu's Vulkan allocations through Mesa. The driver holds them as shmem, and the shell's maps of them (`anon_inode:i915.gem` in `/proc/<pid>/smaps`) count no RSS, so RSS and PSS never show them. DRM fdinfo (`/proc/<pid>/fdinfo/<fd>` of a `/dev/dri` fd, `drm-total-*` and `drm-resident-*`) shows them, once per `drm-client-id`. So does the cgroup's `shmem`.
- **The cgroup** (`memory.current`, `memory.peak`, `memory.stat`): all of the above, the helper processes (`pw-dump`, `udevadm`, `pactl`, `dbus-monitor`, `wl-paste`, `systemd-inhibit`, `awww-daemon`), page cache and kernel slab. The kernel charges page cache and dentries to the cgroup that first reads a file. So the cache and reclaimable slab depend on what was cached before the shell started, not on the shell. Both are reclaimable.

The memory peak journalctl reports when the unit stops is `memory.peak`. Its spread, from 341 MB to 798 MB in the #154 E2E, points to cache and slab: a shell started with the files cached reads about 345 MB, while the installed shell, sampled once at 877 MiB, held 447 MiB of page cache and 128 MiB of reclaimable slab on top.

## Procedure

Needs a release build, every monitor connected, and no other shell running. The lock step waits for your password.

```sh
cargo build --release
systemctl --user stop kanade.service
systemd-run --user --unit=kanade-memory --collect "$PWD/target/release/kanade"
sleep 20                                  # settle at Rest
KANADE=target/release/kanade scripts/memory
systemctl --user stop kanade-memory
systemctl --user start kanade.service
```

The transient unit gives the shell its own cgroup, as `kanade.service` does, so the cgroup columns and journalctl's peak match the installed shell. `scripts/memory` samples at Rest, two seconds after opening each Surface, after collapsing, with the Settings window open and closed, locked, and after you unlock. `scripts/memory sample LABEL` takes one sample of the running shell.

Columns, in MiB: the shell's `rss`, `pss`, `anon` and PSS of mapped files (`file`); `gpu` and `gpu-res`, the GPU buffers it allocated and has resident; then its cgroup's current use, `peak` since start, `anon`, page `cache`, `shmem` (mostly the GPU buffers), reclaimable slab (`slab-r`), and the PSS of the helper processes (`others`).

To include cold cache and slab, as on a first start after boot, drop the caches before starting it: `sync; echo 3 | sudo tee /proc/sys/vm/drop_caches`.

## Baseline

2026-10-09, `main` at 8d69207, release build, files cached. Lenovo laptop, Intel UHD Graphics (Alder Lake-S, i915) driving one monitor, eDP-1 1920x1080 at 144 Hz, scale 1. niri 26.04, Mesa 26.2.4, Linux 7.2.9. The NVIDIA GPU is unused, apart from 1 MiB of wgpu probing it.

```
                   rss     pss    anon    file     gpu gpu-res |  cgroup    peak    anon   cache   shmem  slab-r  others
idle             150.6   124.6    51.9    72.6   224.8   224.5 |   297.5   343.2    57.4     0.0   232.0     0.2    13.7
controls         157.1   131.0    58.4    72.6   224.8   224.5 |   304.9   343.2    63.9     0.0   232.0     0.2    13.7
media            176.9   150.8    78.0    72.7   224.8   224.5 |   342.9   343.9    83.6    18.5   232.0     0.2    13.7
notifications    182.1   156.1    83.3    72.7   224.8   224.5 |   348.3   350.1    88.8    18.5   232.0     0.2    13.7
launcher         183.1   157.1    84.3    72.7   228.8   228.5 |   353.4   355.0    89.8    18.6   236.0     0.2    13.7
tray             181.4   155.4    82.6    72.7   228.8   228.5 |   352.0   355.0    88.1    18.6   236.0     0.2    13.7
clipboard        181.7   155.6    82.9    72.7   228.8   228.5 |   351.4   355.0    88.4    18.6   236.0     0.2    13.7
calendar         219.6   193.5   120.7    72.8   232.8   232.5 |   393.5   394.1   126.2    18.6   240.0     0.2    13.7
weather          219.6   193.6   120.7    72.8   232.8   232.5 |   393.9   396.3   126.2    18.6   240.0     0.2    13.7
collapse         220.5   194.5   121.6    72.8   236.8   236.5 |   399.3   400.0   127.2    18.6   244.0     0.2    13.7
settings         218.8   192.8   119.9    72.8   247.7   247.5 |   408.8   411.2   125.4    18.6   254.9     0.2    13.7
closed           218.8   192.7   119.9    72.8   240.8   240.5 |   401.2   411.2   125.4    18.6   248.0     0.2    13.7
locked           218.9   192.8   120.0    72.8   272.9   272.6 |   432.9   434.6   125.5    18.6   280.0     0.2    13.7
unlocked         219.9   193.5   120.0    73.4   240.8   240.5 |   405.0   455.8   125.5    20.7   248.0     0.2    13.6
```

journalctl: `kanade-memory.service: ... 455.8M memory peak`.

What dominates:

- GPU buffers, 225 MiB at Rest, more than the shell's whole PSS. They grow by 4 MiB with the Launcher, Calendar and collapse, by 11 MiB for the Settings window, 4 of which stay once it closes, and by 32 MiB for the lock screen, which frees them on unlock.
- Anonymous memory, 52 MiB at Rest. Media adds 20 MiB and the Calendar 37 MiB, and neither is freed when the island collapses.
- Mapped files, 73 MiB of PSS, stay flat.
