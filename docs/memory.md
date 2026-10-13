# Memory baseline

How much memory the shell uses, split by kind, and how to measure it again (#198). Take a new baseline before and after any memory optimization, on the same machine, monitor setup and data.

## Where the memory is

- **RSS, PSS, anonymous** (`/proc/<pid>/smaps_rollup`): the shell's own pages. Anonymous is the heap, malloc's other maps and thread stacks. Mapped files are the binary and its libraries, graphics drivers included.
- **GPU buffers**: wgpu's Vulkan allocations. The driver holds them as shmem, and the shell's maps of them (`anon_inode:i915.gem` in `/proc/<pid>/smaps`) count no RSS, so RSS and PSS never show them. DRM fdinfo (`/proc/<pid>/fdinfo/<fd>` of a `/dev/dri` fd, `drm-total-*` and `drm-resident-*`) shows them, once per `drm-client-id` and device (`drm-pdev`), summed over every memory region. A buffer shared between two of the shell's clients counts twice. NVIDIA's proprietary driver has no DRM fdinfo; `nvidia-smi` shows what it holds. Older amdgpu kernels print only the deprecated `drm-memory-*`, which the script does not read.
- **The cgroup** (`memory.current`, `memory.peak`, `memory.stat`): the pages charged to it. These are the shell's anonymous pages, its GPU buffers as `shmem`, the helper processes (`pw-dump`, `udevadm`, `pactl`, `dbus-monitor`, `wl-paste`, `systemd-inhibit`, `awww-daemon`), page cache and kernel slab. The kernel charges a file's pages and dentries to the cgroup that first reads it. So mapped libraries that another process loaded first are not charged, and the cache and reclaimable slab depend on what was cached before the shell started. Both are reclaimable.

The memory peak journalctl reports when the unit stops is `memory.peak`. The shell's own use moves it by about 80 MiB, from 343 MiB at Rest to 423-425 MiB once every Surface and the lock screen have shown. Cold cache moves it further: the installed shell, sampled once at 877 MiB, held 447 MiB of page cache and 128 MiB of reclaimable slab beside the shell's own. That most likely explains the 798 MiB peak in the #154 E2E, which was not sampled.

## Procedure

Needs a release build, every monitor connected, no other shell running, and every Surface's Module on. The lock step waits for your password.

```sh
cargo build --release
systemctl --user stop kanade.service
systemd-run --user --unit=kanade-memory --collect "$PWD/target/release/kanade"
sleep 20                                  # settle at Rest
KANADE=target/release/kanade scripts/memory
systemctl --user stop kanade-memory
systemctl --user start kanade.service
```

The transient unit gives the shell its own cgroup, as `kanade.service` does, so the cgroup columns and journalctl's peak match the installed shell. `scripts/memory` samples at Rest, two seconds after opening each Surface, after collapsing, with the Settings window open and closed, locked, and after you unlock. `scripts/memory sample LABEL` takes one sample; `UNIT=kanade.service` measures the installed shell.

Columns, in MiB: the shell's `rss`, `pss`, `anon` and PSS of mapped files (`file`); `gpu` and `gpu-res`, the GPU buffers it allocated and has resident; then its cgroup's current use, `peak` since start, `anon`, page `cache`, `shmem` (mostly the GPU buffers), reclaimable slab (`slab-r`), and the PSS of the helper processes (`others`). `gpu-res` can drop for a while and recover, with `gpu` and `shmem` unchanged: the installed shell has shown as little as 1 MiB of its 224 MiB of buffers resident, and 218 MiB again a minute later. Why was not traced; the driver may have released the buffers' pages to its shmem cache. Compare `gpu` between runs.

What the Surfaces show changes the numbers, so record it with each baseline: the config, the calendars and their events, the weather location, a media player and its art, the tray items, and notifications and clipboard history (both empty at start, as they are kept only in memory). To include cold cache and slab, as on a first start after boot, drop the caches before starting the shell: `sync; echo 3 | sudo tee /proc/sys/vm/drop_caches`.

## Baseline

2026-10-11, `main` at 39313b7, release build, files cached. The same machine and setup as the 2026-10-09 baseline below (one monitor, no config, calendars, weather location, media player or tray items), with NVIDIA 615.78.08. One run, after GPU glass (ADR 0037), ADR 0025's CPU lock screen, and vello 0.11 with wgpu 30:

```
                   rss     pss    anon    file     gpu gpu-res |  cgroup    peak    anon   cache   shmem  slab-r  others
rest             312.1   302.5   194.6   107.9   495.0   493.0 |   764.8   892.0   200.0    52.9   499.9     0.4    13.8
controls         338.5   328.9   220.9   108.0   501.0   499.0 |   797.8   892.0   226.3    52.9   505.9     0.4    13.8
media            352.1   342.5   234.5   108.0   501.0   499.0 |   811.0   892.0   239.9    52.9   505.9     0.4    13.8
notifications    357.1   347.5   239.5   108.0   501.0   499.0 |   815.8   892.0   244.9    52.9   505.9     0.4    13.8
launcher         362.8   353.3   245.3   108.0   503.0   501.0 |   824.2   892.0   250.7    53.0   507.9     0.4    13.8
tray             357.1   347.5   239.5   108.0   511.0   509.0 |   826.0   892.0   244.9    53.0   515.9     0.4    13.8
clipboard        380.4   370.8   262.8   108.0   511.0   509.0 |   868.2   892.0   268.2    71.2   515.9     0.4    13.8
calendar         406.3   396.8   288.8   108.0   509.0   507.0 |   891.9   892.2   294.2    71.2   513.9     0.4    13.8
weather          409.2   399.7   291.6   108.0   509.0   507.0 |   894.9   895.1   297.1    71.2   513.9     0.4    13.8
collapse         408.5   398.9   290.9   108.0   509.0   507.0 |   894.2   895.1   296.3    71.2   513.9     0.4    13.8
settings         430.1   420.5   312.5   108.0   555.5   553.5 |   962.3   970.2   317.9    71.2   560.3     0.4    13.8
closed           431.2   421.6   313.6   108.0   531.0   529.0 |   939.3   970.2   319.0    71.2   535.9     0.4    13.8
locked           450.5   433.0   316.8   108.2   531.0   529.0 |   957.5   970.2   322.2    71.2   551.7     0.4    13.8
unlocked         437.8   427.8   319.0   108.7   531.0   529.0 |   947.6   980.1   324.4    74.5   535.9     0.4    13.7
```

- GPU buffers more than doubled from 225 to 495 MiB at Rest, and RSS from 150 to 312 MiB. Both come from GPU glass: the build before the vello and wgpu upgrade (`03245ec`) measured 487 MiB of GPU buffers and 275 MiB of RSS at Rest.
- The upgrade itself costs about 26 MiB of PSS at Rest: 15 MiB of heap and 10 MiB for `libnvidia-rtcore`, which the new build loads and the old did not. Compare Rest: the later samples depend on what each Surface allocated, and this baseline is one run.
- The lock screen costs about 15-20 MiB of RSS and no GPU buffers now (ADR 0025); the Settings window 47 MiB of GPU buffers, 22 of them kept once it closes.
- `docs/design.md` once capped RSS at 80 MB and no longer does; Rest RSS is 312 MiB and leaves out the GPU buffers.

## Baseline before GPU glass

2026-10-09, `main` at 8d69207, release build, files cached. Lenovo laptop, Intel UHD Graphics (Alder Lake-S, i915) driving one monitor, eDP-1 1920x1080 at 144 Hz, scale 1. niri 26.04, Mesa 26.2.4, NVIDIA 615.71.09, Linux 7.2.9. No config file, no calendars, Google Calendar signed out, no weather location, no media player, no tray items.

Only one monitor was available, so locked means one lock screen. Take the baseline again with more monitors connected.

Two runs, each in a fresh unit. The first:

```
                   rss     pss    anon    file     gpu gpu-res |  cgroup    peak    anon   cache   shmem  slab-r  others
rest             149.6   123.5    50.5    72.9   224.8   224.5 |   296.4   342.8    56.0     0.0   232.0     0.2    13.6
controls         155.6   129.5    56.5    72.9   224.8   224.5 |   303.0   342.8    62.0     0.0   232.0     0.2    13.6
media            176.3   150.2    77.1    73.1   224.8   224.5 |   323.7   342.8    82.6     0.0   232.0     0.2    13.6
notifications    181.5   155.4    82.3    73.1   224.8   224.5 |   328.6   342.8    87.8     0.0   232.0     0.2    13.6
launcher         181.9   155.8    82.7    73.1   228.8   228.5 |   332.8   342.8    88.2     0.0   236.0     0.2    13.6
tray             180.3   154.2    81.1    73.1   228.8   228.5 |   331.6   342.8    86.6     0.0   236.0     0.2    13.6
clipboard        181.0   154.9    81.8    73.1   228.8   228.5 |   332.9   342.8    87.3     0.0   236.0     0.2    13.6
calendar         213.7   187.6   114.5    73.1   230.8   230.5 |   366.6   366.8   120.0     0.0   238.0     0.2    13.6
weather          213.7   187.6   114.5    73.1   230.8   230.5 |   367.7   367.7   120.0     0.0   238.0     0.2    13.6
collapse         214.7   188.6   115.5    73.1   234.8   234.5 |   372.2   372.8   121.0     0.0   242.0     0.2    13.6
settings         213.2   187.1   114.0    73.1   245.7   245.5 |   382.2   384.1   119.5     0.0   252.9     0.2    13.6
closed           213.2   187.1   114.0    73.1   238.8   238.5 |   374.4   384.1   119.5     0.0   246.0     0.2    13.6
locked           213.2   187.2   114.0    73.1   270.9   270.6 |   407.2   407.4   119.5     0.0   278.0     0.2    13.6
unlocked         214.4   188.0   114.2    73.7   238.8   238.5 |   374.2   425.2   119.7     0.0   246.0     0.2    13.6
```

The second differs by under 4 MiB in every column:

```
rest             149.6   123.5    50.7    72.8   224.8   224.5 |   296.6   344.0    56.2     0.0   232.0     0.2    13.7
unlocked         212.7   186.3   112.7    73.6   238.8   238.5 |   373.7   422.8   118.2     0.0   246.0     0.2    13.7
```

journalctl reported the same peaks for both runs: `425.2M memory peak` and `422.8M memory peak`.

What dominates:

- GPU buffers, 225 MiB at Rest, more than the shell's whole PSS. They grow by 4 MiB with the Launcher and with collapsing, 2 MiB with the Calendar, 11 MiB with the Settings window, 4 MiB of which stay once it closes, and 32 MiB with the lock screen, which frees them on unlock.
- Anonymous memory, 51 MiB at Rest. Media adds 21 MiB and the Calendar 33 MiB, with no calendar to show, and neither is freed when the island collapses.
- Mapped files, 73 MiB of PSS, stay flat. NVIDIA's driver libraries (`libnvidia-gpucomp`, `-glcore`, `-eglcore` and others) cost 35 MiB of PSS, 29 MiB of it mapped files and the rest anonymous, plus 3 MiB for `/dev/nvidiactl`. Mesa's `libLLVM` and `libgallium` cost 17 MiB, 15 MiB of it mapped files. The shell draws on the Intel GPU, whose i915 client holds its buffers and render time, while `nvidia-smi` shows it holding 1 MiB on the NVIDIA one. So the NVIDIA libraries are loaded but barely used. Why either set loads was not traced; one guess, untested, is wgpu probing every Vulkan and EGL driver.
- The peak is above every sample twice. It is 46 MiB over Rest from startup, and 18 MiB over the lock screen during the password check and unlock.

`docs/design.md` once capped RSS at 80 MB and no longer does. The shell's RSS is 150 MiB at Rest and 214 MiB after every Surface, and RSS leaves out the 225 MiB of GPU buffers.
