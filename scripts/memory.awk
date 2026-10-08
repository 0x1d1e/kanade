# one row of scripts/memory: reads its prefixed sample (rollup, fdinfo, others, cgroup lines)
# and prints the columns, in MiB; -v label=LABEL names the row
$1 == "rollup" { rollup[$2] = $3 }
# DRM fdinfo (drm-usage-stats), once per client: the GPU buffers the driver holds for the
# shell, in no RSS, as its maps of them count none. A client id may be unique only per
# device, given with drm-pdev. drm-total-cycles-* is engine time
$1 == "fdinfo" && $3 == "drm-client-id:" { client[$2] = $4 }
$1 == "fdinfo" && $3 == "drm-pdev:" { pdev[$2] = $4 }
$1 == "fdinfo" && $3 ~ /^drm-(total|resident)-/ && $3 !~ /-cycles-/ {
    split($3, key, "-")
    size[$2, key[2]] += unit($4, $5)
}
$1 == "cgroup" { cg[$2] = $3 }
$1 == "others" { others += $2 }
function unit(n, u) { return u == "MiB" ? n * 1024 : u == "GiB" ? n * 1048576 : u == "KiB" ? n : n / 1024 }
function mib(kib) { return sprintf("%7.1f", kib / 1024) }
END {
    for (fd in client) {
        id = pdev[fd] SUBSEP client[fd]
        if (id in seen) continue
        seen[id] = 1
        total += size[fd, "total"]
        resident += size[fd, "resident"]
    }
    printf "%-14s %s %s %s %s %s %s | %s %s %s %s %s %s %s\n", label,
        mib(rollup["Rss:"]), mib(rollup["Pss:"]), mib(rollup["Anonymous:"]), mib(rollup["Pss_File:"]),
        mib(total), mib(resident),
        mib(cg["current"] / 1024), ("peak" in cg ? mib(cg["peak"] / 1024) : sprintf("%7s", "-")), mib(cg["anon"] / 1024),
        mib((cg["file"] - cg["shmem"]) / 1024), mib(cg["shmem"] / 1024),
        mib(cg["slab_reclaimable"] / 1024), mib(others)
}
