import QtQuick 2.15

/*
 * SDDM's entry: a thin adapter from its globals (sddm, userModel, sessionModel, keyboard, config) to
 * the host object Greeter.qml draws from. Every global is checked with typeof, so this runs under a
 * plain qml too, and a failed Greeter.qml leaves a minimal login form: a runtime error here does not
 * fall back to SDDM's own theme, it leaves a blank screen.
 */
Item {
    id: root

    // SDDM resizes this to the screen
    width: 1280
    height: 720

    // ---- the host contract of Greeter.qml
    property var users: []
    property var sessions: []
    property int defaultUser: typeof userModel !== "undefined" && userModel && userModel.lastIndex >= 0 ? userModel.lastIndex : 0
    property int defaultSession: typeof sessionModel !== "undefined" && sessionModel && sessionModel.lastIndex >= 0 ? sessionModel.lastIndex : 0
    property string info: ""
    readonly property bool capsLock: typeof keyboard !== "undefined" && keyboard ? keyboard.capsLock === true : false
    readonly property var layouts: {
        var out = []
        if (typeof keyboard !== "undefined" && keyboard && keyboard.layouts) {
            for (var i = 0; i < keyboard.layouts.length; i++)
                out.push(String(keyboard.layouts[i].shortName || "").toUpperCase())
        }
        return out
    }
    readonly property int layoutIndex: typeof keyboard !== "undefined" && keyboard && keyboard.currentLayout >= 0 ? keyboard.currentLayout : 0
    readonly property bool canSuspend: typeof sddm !== "undefined" && sddm ? sddm.canSuspend === true : false
    readonly property bool canReboot: typeof sddm !== "undefined" && sddm ? sddm.canReboot === true : false
    readonly property bool canPowerOff: typeof sddm !== "undefined" && sddm ? sddm.canPowerOff === true : false

    // SDDM makes one of these per screen, and each is a whole login screen with a live field: which
    // output a person types at is not known here (keys go to the window under the pointer, or the
    // one clicked). The answers of the one `sddm` reach every instance, so every one clears and
    // refocuses; each guards its own submit, and the instances of one engine guard each other's.

    readonly property var options: ({
        backdrop: setting("backdrop"),
        face: setting("face"),
        shade: setting("shade"),
        font: setting("font"),
        clock24: setting("clock24"),
        scale: setting("scale"),
        session: setting("session"),
        surface: setting("surface")
    })

    signal loginFailed()
    signal loginSucceeded()

    function setting(key) {
        return (typeof config !== "undefined" && config) ? config[key] : undefined
    }

    function login(user, password, session) {
        info = ""
        if (typeof sddm !== "undefined" && sddm)
            sddm.login(user, password, session)
    }

    function cycleLayout() {
        if (typeof keyboard !== "undefined" && keyboard && keyboard.layouts && keyboard.layouts.length > 1)
            keyboard.currentLayout = (Math.max(0, keyboard.currentLayout) + 1) % keyboard.layouts.length
    }

    function suspend() {
        if (typeof sddm !== "undefined" && sddm && sddm.canSuspend)
            sddm.suspend()
    }

    function reboot() {
        if (typeof sddm !== "undefined" && sddm && sddm.canReboot)
            sddm.reboot()
    }

    function powerOff() {
        if (typeof sddm !== "undefined" && sddm && sddm.canPowerOff)
            sddm.powerOff()
    }

    function collect() {
        var found = []
        for (var i = 0; i < userRows.count; i++) {
            var row = userRows.itemAt(i)
            if (row)
                found.push({ name: row.who, realName: row.real, icon: row.icon })
        }
        users = found

        var named = []
        for (var j = 0; j < sessionRows.count; j++) {
            var item = sessionRows.itemAt(j)
            if (item)
                named.push(item.label)
        }
        sessions = named
    }

    Component.onCompleted: {
        if (typeof sddm !== "undefined" && sddm) {
            if (sddm.loginFailed)
                sddm.loginFailed.connect(root.loginFailed)
            if (sddm.loginSucceeded)
                sddm.loginSucceeded.connect(root.loginSucceeded)
            if (sddm.informationMessage)
                sddm.informationMessage.connect(function (message) { root.info = String(message) })
        }
        collect()
    }

    // SDDM's models can only be read through a delegate
    Repeater {
        id: userRows
        model: typeof userModel !== "undefined" ? userModel : null
        onItemAdded: root.collect()
        onItemRemoved: root.collect()

        Item {
            visible: false
            readonly property string who: model.name !== undefined ? String(model.name) : ""
            readonly property string real: model.realName !== undefined ? String(model.realName) : ""
            readonly property string icon: model.icon !== undefined ? String(model.icon) : ""
        }
    }

    Repeater {
        id: sessionRows
        model: typeof sessionModel !== "undefined" ? sessionModel : null
        onItemAdded: root.collect()
        onItemRemoved: root.collect()

        Item {
            visible: false
            readonly property string label: model.name !== undefined ? String(model.name) : ""
        }
    }

    Rectangle {
        anchors.fill: parent
        color: "#0c0d11"
    }

    Loader {
        id: greeter
        anchors.fill: parent
        source: "Greeter.qml"
        onLoaded: item.host = root
    }

    Loader {
        anchors.fill: parent
        active: greeter.status === Loader.Error
        sourceComponent: plain
    }

    // the least that still logs in
    Component {
        id: plain

        Item {
            TextInput {
                id: fallbackUser
                visible: root.users.length === 0
                anchors.horizontalCenter: parent.horizontalCenter
                y: parent.height * 0.4 - 40
                width: 240
                height: 24
                color: "#f1f2f3"
                font.pixelSize: 16
                verticalAlignment: TextInput.AlignVCenter
                clip: true
                maximumLength: 256
                Component.onCompleted: if (root.users.length === 0) forceActiveFocus()
                Keys.onReturnPressed: secret.forceActiveFocus()
                Keys.onEnterPressed: secret.forceActiveFocus()
                Keys.onTabPressed: secret.forceActiveFocus()
            }

            Text {
                id: who
                anchors.horizontalCenter: parent.horizontalCenter
                y: parent.height * 0.4
                color: "#f1f2f3"
                font.pixelSize: 16
                text: root.users.length > 0 ? (root.users[root.defaultUser < root.users.length ? root.defaultUser : 0].realName || root.users[0].name) : ""
            }

            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                y: who.y + 36
                width: 240
                height: 36
                radius: 18
                color: "#000000"
                border.width: 1
                border.color: "#40ffffff"

                TextInput {
                    id: secret
                    anchors.fill: parent
                    anchors.margins: 10
                    color: "#f1f2f3"
                    font.pixelSize: 14
                    echoMode: TextInput.Password
                    focus: root.users.length > 0
                    verticalAlignment: TextInput.AlignVCenter
                    Component.onCompleted: if (root.users.length > 0) forceActiveFocus()
                    Keys.onReturnPressed: submit()
                    Keys.onEnterPressed: submit()

                    function submit() {
                        var name = ""
                        if (root.users.length > 0)
                            name = root.users[root.defaultUser < root.users.length ? root.defaultUser : 0].name
                        else
                            name = fallbackUser.text
                        if (name !== "")
                            root.login(name, text, root.defaultSession)
                        text = ""
                    }
                }
            }
        }
    }
}
