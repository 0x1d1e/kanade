import QtQuick 2.15

// a quiet text control: muted words, a faint wash under the pointer, a thin ring under the keyboard
Item {
    id: chip

    property string label: ""
    property string face: "sans-serif"
    property color ink: "#f2f2f7"
    property color muted: "#c2c4c7"
    property color alarm: "#ff453a"

    // the keyboard is on it, and it asks to be pressed again
    property bool focused: false
    property bool armed: false

    // a chip that only tells (one session, say) takes no press
    property bool active: true

    signal activated()

    implicitWidth: words.implicitWidth + 22
    width: implicitWidth
    height: 26

    Rectangle {
        anchors.fill: parent
        radius: height / 2
        color: area.containsMouse && chip.active ? Qt.rgba(1, 1, 1, 0.12) : "transparent"
        border.width: chip.focused ? 1 : 0
        border.color: chip.ink
    }

    Text {
        id: words
        anchors.centerIn: parent
        text: chip.label
        color: chip.armed ? chip.alarm : ((area.containsMouse && chip.active) || chip.focused ? chip.ink : chip.muted)
        font.family: chip.face
        font.pixelSize: 13
        font.weight: Font.Medium
        textFormat: Text.PlainText
    }

    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        enabled: chip.active
        cursorShape: Qt.PointingHandCursor
        onClicked: chip.activated()
    }
}
