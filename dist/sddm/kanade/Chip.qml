import QtQuick 2.15

// a quiet control: a glyph beside muted words, a faint wash under the pointer, a thin ring under
// the keyboard
Item {
    id: chip

    property string label: ""
    property string icon: ""
    property real iconSize: 13
    property real gap: 6
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

    // the ink the glyph and the words take alike, so the two never disagree
    readonly property color told: armed ? alarm : ((area.containsMouse && active) || focused ? ink : muted)

    // tall enough for the glyph and the words both
    readonly property real rowHeight: Math.max(words.implicitHeight, icon !== "" ? iconSize : 0)

    implicitWidth: words.implicitWidth + (icon !== "" ? iconSize + gap : 0) + 22
    width: implicitWidth
    height: 26

    Rectangle {
        anchors.fill: parent
        radius: height / 2
        color: area.containsMouse && chip.active ? Qt.rgba(1, 1, 1, 0.12) : "transparent"
        border.width: chip.focused ? 1 : 0
        border.color: chip.ink
    }

    Row {
        id: content
        anchors.centerIn: parent
        spacing: chip.gap
        height: chip.rowHeight

        Item {
            // a hidden item takes no room in a Row, nor its gap
            visible: chip.icon !== ""
            width: chip.iconSize
            height: content.height

            Glyph {
                anchors.verticalCenter: parent.verticalCenter
                width: chip.iconSize
                height: chip.iconSize
                name: chip.icon
                size: chip.iconSize
                ink: chip.told
            }
        }

        Item {
            width: words.implicitWidth
            height: content.height

            Text {
                id: words
                anchors.verticalCenter: parent.verticalCenter
                text: chip.label
                color: chip.told
                font.family: chip.face
                font.pixelSize: 13
                font.weight: Font.Medium
                textFormat: Text.PlainText
            }
        }
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
