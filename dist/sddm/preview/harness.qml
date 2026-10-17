import QtQuick 2.15

/*
 * Renders Greeter.qml in every state against a mock host, to PNGs, and prints the geometry of what
 * it drew (lines "PROBE {json}") for scripts/sddm-preview to check. Not installed. Arguments after
 * the file name:  theme=<dir of Greeter.qml>  out=<dir for PNGs>  width=<logical>  height=<logical>
 * It runs under qml6 and qml-qt5 alike.
 */
Item {
    id: root

    property string theme: ""
    property string out: "/tmp/sddm-preview"
    property int index: -1

    width: 1920
    height: 1080

    // ---- the mock host
    QtObject {
        id: host
        property var options: ({})
        property var users: [{ name: "kanade", realName: "Kanade", icon: "" }]
        property var sessions: ["Niri", "Plasma (Wayland)", "GNOME"]
        property int defaultUser: 0
        property int defaultSession: 0
        property bool capsLock: false
        property var layouts: []
        property int layoutIndex: 0
        property string info: ""
        property bool canSuspend: false
        property bool canReboot: false
        property bool canPowerOff: false
        property var now: new Date(2026, 9, 16, 9, 41, 12)

        signal loginFailed()
        signal loginSucceeded()

        property var logins: []
        function login(user, password, session) { logins.push({ user: user, session: session, length: password.length }) }
        function cycleLayout() { calls.push("layout"); layoutIndex = (layoutIndex + 1) % Math.max(1, layouts.length) }
        function suspend() { calls.push("suspend") }
        function reboot() { calls.push("reboot") }
        function powerOff() { calls.push("powerOff") }
        property var calls: []
    }

    // a second screen's instance of the greeter, as SDDM makes one per output, on the same host
    Loader {
        id: other
        anchors.fill: parent
        visible: false
        onLoaded: {
            item.host = host
            item.still = true
        }
    }

    Loader {
        id: loader
        anchors.fill: parent
        onLoaded: {
            item.host = host
            item.still = true
            root.apply(root.scenarios[0])
            root.index = 0
        }
    }

    // each state: what to set on the host and the greeter
    readonly property var many: [{ name: "kanade", realName: "Kanade", icon: "" },
                                 { name: "guest", realName: "Guest Account", icon: "" },
                                 { name: "ana", realName: "Ana Maria", icon: "" }]
    readonly property var pictured: ({ backdrop: "data/backdrop.png", face: "data/face.png", shade: "0.33" })
    readonly property var scenarios: [
        { name: "idle" },
        { name: "typing6", typed: 6 },
        { name: "typing30", typed: 30 },
        { name: "caps", typed: 4, host: { capsLock: true } },
        { name: "layouts", typed: 3, host: { capsLock: true, layouts: ["US", "DE"] } },
        { name: "wrong-start", phase: "wrong", shake: 0.0, host: {} },
        { name: "wrong-peak", phase: "wrong", shake: 0.036, host: {} },
        { name: "wrong-mid", phase: "wrong", shake: 0.15, host: {} },
        { name: "wrong-rest", phase: "wrong", shake: 0.5, host: {} },
        { name: "checking", phase: "checking", typed: 6 },
        { name: "info", host: { info: "Touch the fingerprint reader" } },
        { name: "h12", options: { clock24: "false" }, host: { now: new Date(2026, 9, 16, 14, 7, 0) } },
        { name: "h12-midnight", options: { clock24: "false" }, host: { now: new Date(2026, 9, 6, 0, 5, 0) } },
        { name: "longname", host: { users: [{ name: "x", realName: "Maximilian Alexander Konstantin von Hohenzollern-Sigmaringen the Third of Everywhere", icon: "" }] } },
        { name: "pictured", typed: 5, options: root.pictured },
        { name: "pictured-light", options: { backdrop: "data/backdrop-light.png", shade: "0.33" }, typed: 0 },
        { name: "multiuser", host: { users: root.many, defaultUser: 1 }, ring: 1 },
        { name: "chips", host: { users: root.many, canSuspend: true, canReboot: true, canPowerOff: true }, ring: 2 },
        { name: "armed", host: { canSuspend: true, canReboot: true, canPowerOff: true }, ring: 4, armed: "poweroff" },
        { name: "garbage", options: { backdrop: "../../etc/passwd", face: "/etc/passwd", shade: "banana", font: "((((", clock24: "maybe", scale: "NaN", session: "nonexistent", surface: "#zzzzzz" } },
        { name: "garbage2", options: { backdrop: "data/missing.png", face: "..\\..\\x", shade: "-5", font: "Nonexistent Font Family", clock24: 7, scale: "99999", surface: "red", session: "" } },
        // an absolute backdrop is read from the overlay's directory and nowhere else; the file is not
        // there, so only the path the theme settled on is checked
        { name: "bg-overlay", options: { backdrop: "/usr/local/share/kanade-sddm/bg/backdrop.png" } },
        { name: "bg-overlay-webp", options: { backdrop: "/usr/local/share/kanade-sddm/bg/a-b_c.1.webp" } },
        { name: "bg-outside", options: { backdrop: "/etc/passwd.png" } },
        { name: "bg-prefix-sibling", options: { backdrop: "/usr/local/share/kanade-sddm-evil/bg/backdrop.png" } },
        { name: "bg-dotdot", options: { backdrop: "/usr/local/share/kanade-sddm/../../../etc/shadow.png" } },
        { name: "bg-dotdot-inside", options: { backdrop: "/usr/local/share/kanade-sddm/bg/../backdrop.png" } },
        { name: "bg-empty-segment", options: { backdrop: "/usr/local/share/kanade-sddm/bg//backdrop.png" } },
        { name: "bg-bare-prefix", options: { backdrop: "/usr/local/share/kanade-sddm/" } },
        { name: "bg-extension", options: { backdrop: "/usr/local/share/kanade-sddm/bg/backdrop.svg" } },
        { name: "bg-space", options: { backdrop: "/usr/local/share/kanade-sddm/bg/back drop.png" } },
        { name: "bg-scheme", options: { backdrop: "file:///usr/local/share/kanade-sddm/bg/backdrop.png" } },
        { name: "bg-long", options: { backdrop: "/usr/local/share/kanade-sddm/" + "a".repeat(120) + ".png" } },
        { name: "scale-small", options: { scale: "0.5" } },
        { name: "scale-big", options: { scale: "2" } },
        { name: "empty", host: { users: [], sessions: [] } },
        { name: "second-screen", options: { backdrop: "data/backdrop.png" }, host: { layouts: ["US", "DE"] } }
    ]

    function apply(scenario) {
        var defaults = {
            options: ({}), users: [{ name: "kanade", realName: "Kanade", icon: "" }],
            sessions: ["Niri", "Plasma (Wayland)", "GNOME"], defaultUser: 0, defaultSession: 0,
            capsLock: false, layouts: [], layoutIndex: 0, info: "", canSuspend: false,
            canReboot: false, canPowerOff: false, now: new Date(2026, 9, 16, 9, 41, 12)
        }
        var key
        for (key in defaults)
            host[key] = defaults[key]
        for (key in scenario.host)
            host[key] = scenario.host[key]
        host.options = scenario.options ? scenario.options : ({})

        var g = loader.item
        g.reset()
        g.now = host.now
        g.phase = scenario.phase ? scenario.phase : ""
        g.shakeSince = scenario.shake !== undefined ? scenario.shake : 0.5
        g.armed = scenario.armed ? scenario.armed : ""
        g.password = ""
        var text = ""
        for (var i = 0; i < (scenario.typed ? scenario.typed : 0); i++)
            text += "x"
        g.password = text
        g.ring = scenario.ring ? scenario.ring : 0
    }

    // two screens, one host: one login at a time, both hear the answer, both stay live; and the chips
    // that act on a key press keep the ring so Restart confirms with a second press
    function multi() {
        var a = loader.item
        var b = other.item
        var out = {}
        function reset(c) { c.reset(); c.phase = ""; c.password = ""; c.armed = ""; c.ring = 0 }
        host.users = [{ name: "kanade", realName: "Kanade", icon: "" }]
        host.options = ({})
        host.layouts = ["US", "DE"]
        host.layoutIndex = 0
        host.canSuspend = true
        host.canReboot = true
        host.canPowerOff = true
        host.logins = []
        host.calls = []
        reset(a)
        reset(b)
        a.password = "aaa"
        b.password = "bbb"
        a.submit()
        b.submit()
        out.oneLogin = host.logins.length
        out.bothChecking = a.phase === "checking" && b.phase === "checking"
        host.loginFailed()
        out.bothWrong = a.phase === "wrong" && b.phase === "wrong"
        out.bothCleared = a.password === "" && b.password === ""
        out.bothLive = a.probe().focus && b.probe().focus && a.probe().writable && b.probe().writable
        b.password = "ccc"
        b.submit()
        out.nextLogin = host.logins.length
        out.nextBy = host.logins.length > 1 ? host.logins[1].length : 0
        host.loginFailed()
        a.password = "x"
        for (var i = 0; i < 4097; i++)
            b.password = b.password + "y"
        out.long = b.password.length

        // the keys' ring: Restart armed by one press and confirmed by the next, ring kept
        reset(a)
        var at = 0
        for (var k = 0; k < a.chips.length; k++)
            if (a.chips[k].key === "reboot")
                at = k + 1
        a.ring = at
        a.activate("reboot", true)
        out.armedAfterFirst = a.armed === "reboot" && host.calls.indexOf("reboot") < 0
        out.ringKept = a.ring === at
        a.activate("reboot", true)
        out.rebooted = host.calls.indexOf("reboot") >= 0 && a.armed === ""
        // a click goes back to the password, and a click on Restart arms it, a second one restarts
        a.ring = at
        a.activate("suspend", false)
        out.clickResets = a.ring === 0
        reset(a)
        host.calls = []
        a.ring = at
        a.activate("reboot", false)
        out.clickArms = a.armed === "reboot" && a.ring === 0 && host.calls.indexOf("reboot") < 0
        a.activate("reboot", false)
        out.clickConfirms = host.calls.indexOf("reboot") >= 0 && a.armed === ""
        // Tab away disarms
        reset(a)
        a.ring = at
        a.activate("reboot", true)
        a.ring = at + 1
        out.tabDisarms = a.armed === ""
        // the layout chip cycles the host's layout
        a.activate("layout", true)
        out.layoutCycled = host.layoutIndex === 1
        console.log("MULTI " + JSON.stringify(out))
    }

    // one state at a time: apply it, let it draw, grab it, then the next. `index` is the one shown
    Timer {
        id: step
        interval: 700
        running: root.index >= 0
        onTriggered: {
            var done = root.scenarios[root.index]
            loader.grabToImage(function (result) {
                var probe = loader.item.probe()
                probe.scenario = done.name
                result.saveToFile(root.out + "/" + done.name + ".png")
                console.log("PROBE " + JSON.stringify(probe))
                root.index = root.index + 1
                if (root.index >= root.scenarios.length) {
                    root.multi()
                    finish.start()
                    return
                }
                root.apply(root.scenarios[root.index])
                step.restart()
            })
        }
    }

    Timer {
        id: finish
        interval: 800
        onTriggered: Qt.quit()
    }

    Component.onCompleted: {
        var args = Qt.application.arguments
        for (var i = 0; i < args.length; i++) {
            var pair = String(args[i]).split("=")
            if (pair.length < 2)
                continue
            var value = pair.slice(1).join("=")
            if (pair[0] === "theme")
                theme = value
            else if (pair[0] === "out")
                out = value
            else if (pair[0] === "width")
                width = parseInt(value)
            else if (pair[0] === "height")
                height = parseInt(value)
        }
        loader.source = "file://" + theme + "/Greeter.qml"
        other.source = "file://" + theme + "/Greeter.qml"
        if (loader.status !== Loader.Ready && loader.status !== Loader.Loading) {
            console.log("PROBE-ERROR could not load " + theme)
            Qt.quit()
        }
    }
}
