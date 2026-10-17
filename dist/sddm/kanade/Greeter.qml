import QtQuick 2.15
import "Sanitize.js" as S

/*
 * Kanade's login screen, drawn as its lock screen is (crates/lock/src/paint.rs): the date over a
 * large time near the top; the signed-in user, a black pill to type the password in and what became
 * of the last one near the bottom. Sizes are in logical pixels.
 *
 * It touches no SDDM global: everything comes from `host`, an object that has
 *   options           object: backdrop, face, shade, font, clock24, scale, session, surface (strings)
 *   users             [{name, realName, icon}]
 *   defaultUser       index of the user to start on
 *   sessions          [string], and defaultSession, the index to start on
 *   capsLock          bool
 *   layouts           [string] short labels, and layoutIndex
 *   info              string, a message from the login system
 *   canSuspend, canReboot, canPowerOff   bool
 *   now               Date, optional (a test fixes the clock)
 *   login(user, password, sessionIndex), suspend(), reboot(), powerOff(), cycleLayout()
 * Every screen gets one of these, each with its own live field: SDDM has no notion of which screen
 * a person types at, and keys go to the window under the pointer or the one last clicked.
 *   signals loginFailed(), loginSucceeded()
 * so a preview can mock it.
 */
Item {
    id: root

    property var host: null

    // a test: no blink, no timers, no running animation; the shake is set by `shakeSince`
    property bool still: false

    // what to say under the field: "" nothing, "wrong", "checking"
    property string phase: ""

    // seconds since the last refusal; 0.5 and over is at rest
    property real shakeSince: 0.5

    // the key ring: 0 is the field, 1 and up the chips in order
    property int ring: 0

    // a power button asked once, to be asked again
    property string armed: ""

    property alias password: entry.text

    property int userIndex: 0
    property int sessionIndex: 0

    // ---- what the host says, each with a default so any gap leaves a usable screen
    readonly property var cfg: (host && host.options) ? host.options : ({})
    readonly property var users: (host && host.users && host.users.length) ? host.users : []
    readonly property var sessions: (host && host.sessions && host.sessions.length) ? host.sessions : []
    readonly property var layouts: (host && host.layouts && host.layouts.length) ? host.layouts : []
    readonly property int layoutIndex: (layouts.length > 0 && host) ? Math.max(0, Math.min(layouts.length - 1, Math.floor(Number(host.layoutIndex) || 0))) : 0
    readonly property string layoutName: layouts.length > 1 ? String(layouts[layoutIndex]) : ""
    readonly property bool caps: host ? host.capsLock === true : false
    readonly property string info: (host && typeof host.info === "string") ? host.info : ""
    readonly property bool canSuspend: host ? host.canSuspend === true : false
    readonly property bool canReboot: host ? host.canReboot === true : false
    readonly property bool canPowerOff: host ? host.canPowerOff === true : false

    // ---- the config, checked
    readonly property real ui: S.num(cfg.scale, 1, 0.5, 4)
    readonly property real shade: S.num(cfg.shade, 0.33, 0, 1)
    readonly property bool clock24: S.bool(cfg.clock24, true)
    readonly property string surface: S.hex(cfg.surface, "#0c0d11")
    readonly property string backdropPath: S.backdrop(cfg.backdrop)
    readonly property string facePath: S.path(cfg.face)

    // the families the system has; where Qt cannot say, the chain is left to its own fallback
    readonly property var installed: (typeof Qt.fontFamilies === "function") ? Qt.fontFamilies() : null
    readonly property string chosen: S.family(cfg.font)
    readonly property string face: installed ? S.pick([chosen, "Inter"], installed, "sans-serif") : (chosen !== "" ? chosen : "Inter")
    readonly property string display: installed ? S.pick([chosen, "Inter Display", "Inter"], installed, "sans-serif") : (chosen !== "" ? chosen : "Inter Display")

    // over a wallpaper the island's own light text; over the solid color the slate's
    readonly property bool pictured: backdrop.status === Image.Ready
    readonly property color text: pictured ? "#f2f2f7" : "#f1f2f3"
    readonly property color muted: pictured ? Qt.rgba(235 / 255, 235 / 255, 245 / 255, 184 / 255) : "#c2c4c7"
    readonly property color critical: "#ff453a"

    readonly property bool busy: phase === "checking"

    readonly property var user: (users.length > 0 && userIndex >= 0 && userIndex < users.length) ? users[userIndex] : null
    readonly property string userName: user ? String(user.name || "") : ""
    readonly property string shown: user ? String(user.realName || user.name || "") : ""
    readonly property string sessionName: (sessionIndex >= 0 && sessionIndex < sessions.length) ? String(sessions[sessionIndex]) : ""

    // the date and time, now
    property date now: new Date()

    // the lock screen's numbers
    readonly property real fieldWidth: 240
    readonly property real fieldHeight: 36
    readonly property real statusHeight: 20
    readonly property real avatarSize: 72
    readonly property real gap: 20
    readonly property real dot: 3.5
    readonly property real pitch: 11
    readonly property real inset: fieldHeight / 2
    readonly property real shakeAmplitude: 12

    // ---- layout, in the content's own logical pixels
    readonly property real viewWidth: width / ui
    readonly property real viewHeight: height / ui
    readonly property real middle: viewWidth / 2
    readonly property real dateBaseline: viewHeight * 0.09 + dateLine.ascent
    readonly property real statusTop: viewHeight * 0.92 - statusHeight
    readonly property real fieldTop: statusTop - gap - fieldHeight
    readonly property real nameHeight: nameLine.ascent + nameLine.descent
    readonly property real nameTop: fieldTop - gap - nameHeight
    readonly property real avatarTop: nameTop - gap - avatarSize

    // how far the field is shaken aside now
    readonly property real shaken: {
        var t = shakeSince / 0.5
        if (t >= 1 || t < 0)
            return 0
        return shakeAmplitude * Math.sin(t * 3.5 * 2 * Math.PI) * (1 - t) * (1 - t)
    }

    // the chips under the column, in key order
    readonly property var chips: {
        var list = []
        if (users.length > 1)
            list.push({ key: "user", label: "Switch user", icon: "user", active: true })
        if (sessions.length > 0)
            list.push({ key: "session", label: sessionName, icon: "session", active: sessions.length > 1 })
        if (layouts.length > 1)
            list.push({ key: "layout", label: "Keyboard " + layoutName, icon: "keyboard", active: true })
        if (canSuspend)
            list.push({ key: "suspend", label: "Sleep", icon: "sleep", active: true })
        if (canReboot)
            list.push({ key: "reboot", label: armed === "reboot" ? "Confirm restart" : "Restart", icon: "restart", active: true })
        if (canPowerOff)
            list.push({ key: "poweroff", label: armed === "poweroff" ? "Confirm power off" : "Power Off", icon: "power", active: true })
        return list
    }

    // the dots the field shows
    readonly property int dots: S.fit(entry.text.length, fieldWidth, inset, dot, pitch,
                                      (caps ? 22 : 0) + (layouts.length > 1 ? 32 : 0))

    function pick(wanted) {
        var found = -1
        for (var i = 0; i < users.length; i++)
            if (users[i].name === wanted)
                found = i
        return found
    }

    // SDDM's models fill in after the host is set, so the starting user and session are picked
    // again as they arrive, until the person at the screen has chosen one
    property bool picked: false

    function pickUser() {
        var first = host ? Math.floor(Number(host.defaultUser)) : 0
        userIndex = (isFinite(first) && first >= 0 && first < users.length) ? first : 0
    }

    function pickSession() {
        sessionIndex = S.session(cfg.session, sessions, host ? host.defaultSession : 0)
    }

    function reset() {
        picked = false
        pickUser()
        pickSession()
    }

    readonly property var hostUser: host ? host.defaultUser : 0
    readonly property var hostSession: host ? host.defaultSession : 0
    onUsersChanged: if (!picked) pickUser()
    onHostUserChanged: if (!picked) pickUser()
    onSessionsChanged: if (!picked) pickSession()
    onHostSessionChanged: if (!picked) pickSession()

    function focusField() {
        ring = 0
        entry.forceActiveFocus()
    }

    // a login in flight blocks the submit of every instance in this engine (see Sanitize.js), and
    // this one's own through `busy`; it lets go on an answer, or after the time PAM may take
    readonly property int patience: 30000

    function release() {
        S.release()
        giveUp.stop()
    }

    function submit() {
        if (busy)
            return
        armed = ""
        phase = "checking"
        if (!still)
            giveUp.restart()
        if (S.held(Date.now(), patience))
            return
        S.hold(Date.now())
        if (host && typeof host.login === "function")
            host.login(userName, entry.text, sessionIndex)
    }

    function failed() {
        release()
        phase = "wrong"
        entry.text = ""
        if (!still)
            shaking.restart()
        focusField()
    }

    function succeeded() {
        // PAM said yes: no second submit while the session starts
        phase = "checking"
        S.hold(Date.now())
        if (!still)
            giveUp.restart()
    }

    // `byKey`: a chip pressed with the keys stays focused, so Restart can be confirmed with a second
    // Enter and the session chip cycled with repeated ones; a click returns to the password
    function activate(key, byKey) {
        if (key === "user") {
            picked = true
            if (users.length > 1) {
                userIndex = (userIndex + 1) % users.length
                entry.text = ""
                phase = ""
            }
        } else if (key === "session") {
            picked = true
            if (sessions.length > 1)
                sessionIndex = (sessionIndex + 1) % sessions.length
        } else if (key === "layout") {
            if (host && typeof host.cycleLayout === "function")
                host.cycleLayout()
        } else if (key === "suspend") {
            if (host && typeof host.suspend === "function")
                host.suspend()
        } else if (armed === key) {
            armed = ""
            if (host && key === "reboot" && typeof host.reboot === "function")
                host.reboot()
            else if (host && key === "poweroff" && typeof host.powerOff === "function")
                host.powerOff()
        } else {
            armed = key
            disarm.restart()
        }
        if (byKey)
            entry.forceActiveFocus()
        else
            focusField()
    }

    // the keys moving on to another chip leave nothing armed; going back to the field (a click does
    // that too) keeps it, and Tab round to the field disarms in the key handler
    onRingChanged: {
        if (armed !== "" && ring > 0 && (ring > chips.length || chips[ring - 1].key !== armed))
            armed = ""
    }

    // geometry for a test to read, in the root's pixels
    function probe() {
        var box = function (item) {
            var p = item.mapToItem(root, 0, 0)
            return { x: p.x, y: p.y, width: item.width, height: item.height }
        }
        return {
            ui: ui, width: width, height: height, layouts: layouts.length, phase: phase, held: S.held(Date.now(), patience),
            date: { box: box(dateLine), baseline: dateLine.y + dateLine.baselineOffset, ascent: dateLine.ascent, descent: dateLine.descent, size: dateLine.size, text: dateLine.text },
            time: { box: box(timeLine), baseline: timeLine.y + timeLine.ascent, cap: timeLine.cap, text: timeLine.text },
            status: { box: box(statusLine), baseline: statusLine.y + statusLine.baselineOffset, cap: statusLine.cap, text: statusLine.text, visible: statusLine.visible },
            field: box(field),
            name: { box: box(nameLine), baseline: nameLine.y + nameLine.baselineOffset, ascent: nameLine.ascent, descent: nameLine.descent, text: nameLine.text },
            avatar: box(avatarItem),
            focus: entry.focus, writable: !entry.readOnly, dots: dots, typed: entry.text.length, caps: caps, shaken: shaken,
            caret: { visible: caret.visible, box: box(caret) },
            placeholder: { visible: placeholder.visible, x: placeholder.mapToItem(root, 0, 0).x, baseline: placeholder.y + placeholder.baselineOffset },
            chips: chipRow.children.length,
            backdrop: { path: backdropPath, source: String(backdrop.source) }
        }
    }

    // the host is set once loaded, so its signals are joined here
    property var joined: null

    function join() {
        if (!host || host === joined)
            return
        joined = host
        if (host.loginFailed && typeof host.loginFailed.connect === "function")
            host.loginFailed.connect(failed)
        if (host.loginSucceeded && typeof host.loginSucceeded.connect === "function")
            host.loginSucceeded.connect(succeeded)
        if (host.now)
            now = host.now
    }

    onHostChanged: {
        join()
        reset()
        focusField()
    }
    onUserIndexChanged: entry.forceActiveFocus()

    Component.onCompleted: {
        join()
        reset()
        focusField()
    }

    // the field takes focus once shown, and after any press on the screen
    Timer {
        interval: 120
        running: !root.still
        repeat: false
        onTriggered: root.focusField()
    }

    // the clock changes on the minute
    Timer {
        id: minute
        running: !root.still
        repeat: true
        interval: 1000
        onTriggered: {
            root.now = new Date()
            interval = 60000 - (root.now.getSeconds() * 1000 + root.now.getMilliseconds()) + 50
        }
    }

    NumberAnimation {
        id: shaking
        target: root
        property: "shakeSince"
        from: 0
        to: 0.5
        duration: 500
    }

    // no answer from the login system: let the person try again
    Timer {
        id: giveUp
        interval: root.patience
        onTriggered: {
            S.release()
            if (root.phase === "checking")
                root.phase = ""
        }
    }

    Timer {
        id: disarm
        interval: 4000
        onTriggered: root.armed = ""
    }

    // ---- the backdrop
    Rectangle {
        anchors.fill: parent
        color: root.surface
    }

    Image {
        id: backdrop
        anchors.fill: parent
        source: root.backdropPath === "" ? "" : (root.backdropPath.charAt(0) === "/" ? "file://" + root.backdropPath : Qt.resolvedUrl(root.backdropPath))
        fillMode: Image.PreserveAspectCrop
        smooth: true
        cache: false
        asynchronous: true
        visible: status === Image.Ready
    }

    Rectangle {
        anchors.fill: parent
        visible: root.pictured
        color: Qt.rgba(0, 0, 0, root.shade)
    }

    MouseArea {
        anchors.fill: parent
        onPressed: {
            root.armed = ""
            root.focusField()
        }
    }

    // ---- the screen, whole on every output
    Item {
        id: view
        width: root.viewWidth
        height: root.viewHeight
        scale: root.ui
        transformOrigin: Item.TopLeft

        Line {
            id: dateLine
            text: S.date(root.now)
            face: root.face
            ink: root.text
            size: 24
            weight: Font.DemiBold
            base: root.dateBaseline
            x: root.middle - width / 2
        }

        TimeLine {
            id: timeLine
            text: S.time(root.now, root.clock24)
            face: root.display
            ink: root.text
            size: 112
            weight: Font.DemiBold
            base: root.dateBaseline + dateLine.descent + 8 + cap
            x: root.middle - width / 2
        }

        // the avatar, and the user switcher under a click on it or the name
        Avatar {
            id: avatarItem
            x: root.middle - width / 2
            y: root.avatarTop
            face: root.face
            ink: root.text
            initial: root.shown.length > 0 ? root.shown.charAt(0).toUpperCase() : ""
            sources: {
                var list = []
                if (root.facePath !== "" && root.userIndex === Math.max(0, Math.floor(Number(root.host ? root.host.defaultUser : 0))))
                    list.push(Qt.resolvedUrl(root.facePath))
                if (root.user && root.user.icon)
                    list.push(String(root.user.icon).indexOf("/") === 0 ? "file://" + root.user.icon : String(root.user.icon))
                return list
            }
        }

        Line {
            id: nameLine
            text: root.shown
            face: root.face
            ink: root.text
            size: 15
            weight: Font.DemiBold
            maxWidth: Math.min(480, root.viewWidth - 48)
            base: root.nameTop + ascent
            x: root.middle - width / 2
        }

        MouseArea {
            enabled: root.users.length > 1
            x: Math.min(avatarItem.x, nameLine.x)
            y: avatarItem.y
            width: Math.max(avatarItem.width, nameLine.width)
            height: nameLine.y + nameLine.height - avatarItem.y
            cursorShape: Qt.PointingHandCursor
            onClicked: root.activate("user", false)
        }

        // the field, shaken aside after a password is refused
        Pill {
            id: field
            x: root.middle - root.fieldWidth / 2 + root.shaken
            y: root.fieldTop
            width: root.fieldWidth
            height: root.fieldHeight

            Repeater {
                model: root.dots

                Rectangle {
                    x: root.inset + index * root.pitch
                    y: field.height / 2 - root.dot
                    width: root.dot * 2
                    height: root.dot * 2
                    radius: root.dot
                    color: root.text
                }
            }

            Line {
                id: placeholder
                visible: entry.text.length === 0
                text: "Enter password"
                face: root.face
                ink: root.muted
                size: 14
                maxWidth: root.fieldWidth - root.inset * 2
                x: root.inset + 4
                base: field.height / 2 + cap / 2
            }

            Rectangle {
                id: caret
                width: 1.5
                height: 18
                x: root.dots === 0 ? root.inset : root.inset + (root.dots - 1) * root.pitch + root.dot * 2 + 3
                y: field.height / 2 - 9
                color: root.text
                visible: root.ring === 0 && !root.busy && (root.still || blink.on)
            }

            Timer {
                id: blink
                property bool on: true
                interval: 530
                repeat: true
                running: !root.still && root.ring === 0 && !root.busy
                onTriggered: on = !on
                onRunningChanged: on = true
            }

            // Caps Lock, as macOS's arrow at the field's end, and the keyboard layout before it
            CapsGlyph {
                id: capsGlyph
                visible: root.caps
                size: 16
                ink: root.muted
                x: field.width - root.inset - width
                y: field.height / 2 - height / 2
            }

            Line {
                id: layoutLine
                visible: root.layouts.length > 1
                text: root.layoutName
                face: root.face
                ink: root.muted
                size: 12
                weight: Font.DemiBold
                maxWidth: 30
                x: field.width - root.inset - (root.caps ? 22 : 0) - width
                base: field.height / 2 + cap / 2
            }

            // what is typed lands here, whatever else the keys ring; it is never drawn
            TextInput {
                id: entry
                opacity: 0
                width: 1
                height: 1
                echoMode: TextInput.Password
                maximumLength: 4096
                readOnly: root.busy
                focus: true
                inputMethodHints: Qt.ImhSensitiveData | Qt.ImhNoPredictiveText | Qt.ImhNoAutoUppercase

                Keys.onPressed: function (event) {
                    var key = event.key
                    if (key === Qt.Key_Tab || key === Qt.Key_Backtab) {
                        var count = root.chips.length + 1
                        var back = key === Qt.Key_Backtab || (event.modifiers & Qt.ShiftModifier)
                        root.ring = (root.ring + (back ? count - 1 : 1)) % count
                        if (root.ring === 0)
                            root.armed = ""
                        event.accepted = true
                    } else if (root.ring > 0 && (key === Qt.Key_Left || key === Qt.Key_Right || key === Qt.Key_Up || key === Qt.Key_Down)) {
                        var n = root.chips.length
                        var step = (key === Qt.Key_Left || key === Qt.Key_Up) ? n - 1 : 1
                        root.ring = (root.ring - 1 + step) % n + 1
                        event.accepted = true
                    } else if (key === Qt.Key_Return || key === Qt.Key_Enter) {
                        if (root.ring > 0 && root.ring <= root.chips.length)
                            root.activate(root.chips[root.ring - 1].key, true)
                        else
                            root.submit()
                        event.accepted = true
                    } else if (key === Qt.Key_Space && root.ring > 0) {
                        if (root.ring <= root.chips.length)
                            root.activate(root.chips[root.ring - 1].key, true)
                        event.accepted = true
                    } else if (key === Qt.Key_Escape) {
                        if (root.ring > 0) {
                            root.ring = 0
                            root.armed = ""
                        } else {
                            entry.text = ""
                        }
                        event.accepted = true
                    } else if (root.ring > 0 && event.text.length > 0) {
                        // typing while a chip rings goes back to the password
                        root.ring = 0
                    }
                }
            }
        }

        // what became of the last password
        Line {
            id: statusLine
            readonly property string said: root.phase === "wrong" ? "Wrong password"
                : root.phase === "checking" ? "Checking…" : root.info
            visible: said !== ""
            text: said
            face: root.face
            ink: root.phase === "wrong" ? root.critical : root.muted
            size: 14
            weight: Font.Medium
            maxWidth: Math.min(560, root.viewWidth - 48)
            x: root.middle - width / 2
            base: root.statusTop + root.statusHeight / 2 + cap / 2
        }

        // the quiet controls below it
        Row {
            id: chipRow
            spacing: 4
            x: root.middle - width / 2
            y: root.viewHeight * 0.92 + Math.min(24, (root.viewHeight * 0.08 - 26) / 2)

            Repeater {
                model: root.chips

                Chip {
                    label: modelData.label
                    icon: modelData.icon
                    active: modelData.active
                    face: root.face
                    ink: root.text
                    muted: root.muted
                    alarm: root.critical
                    focused: root.ring === index + 1
                    armed: (modelData.key === "reboot" || modelData.key === "poweroff") && root.armed === modelData.key
                    onActivated: root.activate(modelData.key, false)
                }
            }
        }
    }
}
