// Reads of theme.conf values. Every one takes whatever the config holds (undefined, an empty or a
// garbage string, a number) and gives back a usable value: a bad value must never break the screen.
.pragma library

// a login in flight, as the time it was sent (0 for none). A pragma library is one per QML engine:
// every instance of the theme in an engine shares it, and one's submit holds the others'. If SDDM
// gives each screen its own engine, each instance still guards itself (`busy`).
var sent = 0

function hold(now) {
    sent = now
}

function release() {
    sent = 0
}

function held(now, patience) {
    return sent > 0 && now - sent < patience
}

function str(value, fallback) {
    return (typeof value === "string" || typeof value === "number") ? String(value).trim() : fallback
}

function num(value, fallback, least, most) {
    var text = str(value, "")
    if (text === "" || !/^[-+]?(\d+\.?\d*|\.\d+)$/.test(text))
        return fallback
    var n = parseFloat(text)
    if (!isFinite(n))
        return fallback
    return Math.max(least, Math.min(most, n))
}

function bool(value, fallback) {
    var text = str(value, "").toLowerCase()
    if (text === "true" || text === "1" || text === "yes" || text === "on")
        return true
    if (text === "false" || text === "0" || text === "no" || text === "off")
        return false
    return fallback
}

function hex(value, fallback) {
    var text = str(value, "")
    return /^#[0-9a-fA-F]{6}$/.test(text) ? text.toLowerCase() : fallback
}

// a path inside the theme's directory: relative, no `..`, no scheme, plain characters; else ""
function path(value) {
    var text = str(value, "")
    if (text === "" || text.length > 128)
        return ""
    if (!/^[A-Za-z0-9_][A-Za-z0-9_. \/-]*$/.test(text))
        return ""
    var parts = text.split("/")
    for (var i = 0; i < parts.length; i++)
        if (parts[i] === "" || parts[i] === "." || parts[i] === "..")
            return ""
    return text
}

// the one mutable data file outside the root-owned theme tree
var backdropFile = "/usr/local/share/kanade-sddm-data/backdrop.png"

// a backdrop may be this fixed mutable data file, or a plain relative file shipped in the theme
function backdrop(value) {
    var text = str(value, "")
    if (text === backdropFile)
        return text
    return path(text)
}

// a family name: plain characters, not absurdly long; else ""
function family(value) {
    var text = str(value, "")
    return (text.length > 0 && text.length <= 64 && /^[A-Za-z0-9][A-Za-z0-9 _.+-]*$/.test(text)) ? text : ""
}

// the first of `wanted` that `installed` lists, else `last`
function pick(wanted, installed, last) {
    for (var i = 0; i < wanted.length; i++)
        if (wanted[i] !== "" && installed.indexOf(wanted[i]) >= 0)
            return wanted[i]
    return last
}

// a session index: the one named `wanted` (any case), else `last`, kept inside `names`
function session(wanted, names, last) {
    var want = str(wanted, "").toLowerCase()
    if (want !== "")
        for (var i = 0; i < names.length; i++)
            if (String(names[i]).toLowerCase() === want)
                return i
    var n = Math.floor(Number(last))
    if (!isFinite(n) || n < 0 || n >= names.length)
        return 0
    return n
}

// the date as the lock screen sets it, in English whatever the locale: "Friday, 16 October"
function date(now) {
    var days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]
    var months = ["January", "February", "March", "April", "May", "June", "July", "August",
                  "September", "October", "November", "December"]
    return days[now.getDay()] + ", " + now.getDate() + " " + months[now.getMonth()]
}

// 24h "HH:MM", 12h "H:MM AM"
function time(now, clock24) {
    var minutes = now.getMinutes()
    var tail = (minutes < 10 ? "0" : "") + minutes
    var hours = now.getHours()
    if (clock24)
        return (hours < 10 ? "0" : "") + hours + ":" + tail
    return (hours % 12 === 0 ? 12 : hours % 12) + ":" + tail + (hours < 12 ? " AM" : " PM")
}

// how many of `typed` dots the field shows: as many as fit, `room` being what the caps lock
// arrow and layout label take at its end
function fit(typed, width, inset, dot, pitch, room) {
    var space = width - inset * 2 - room
    var fits = Math.floor(Math.max(0, (space - dot * 2) / pitch)) + 1
    return Math.min(typed, fits)
}

// the soft shadow as stacked translucent layers whose alphas compose a gaussian edge of `blur`/3
// and `peak` opacity: [{grow, alpha}], outermost first
function shadow(blur, steps, peak) {
    var sigma = blur / 3
    var layers = []
    var previous = 0
    var erf = function (x) {
        var t = 1 / (1 + 0.3275911 * Math.abs(x))
        var y = 1 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592)
                * t * Math.exp(-x * x)
        return x >= 0 ? y : -y
    }
    for (var i = 0; i < steps; i++) {
        var grow = blur - (2 * blur) * i / (steps - 1)
        var target = peak * 0.5 * (1 + erf(-grow / (sigma * Math.SQRT2)))
        var alpha = previous >= 1 ? 0 : 1 - (1 - target) / (1 - previous)
        layers.push({ grow: grow, alpha: Math.max(0, Math.min(1, alpha)) })
        previous = target
    }
    return layers
}
