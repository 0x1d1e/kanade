import QtQuick 2.15
import "Sanitize.js" as S

// a pill of the shell's black, with the soft shadow its panes cast: a few translucent rounded
// rectangles stacked to a gaussian edge, so it needs no effect and shows on any renderer. The
// shadow is decoration only, nothing depends on it
Item {
    id: pill

    property real radius: height / 2
    property color fill: "#000000"
    property color rim: Qt.rgba(1, 1, 1, 64 / 255)

    // how far the shadow spreads, how far down it falls, and how dark it is at most
    property real blur: 12
    property real drop: 4
    property real darkness: 56 / 255

    readonly property var layers: S.shadow(blur, 13, darkness)

    Repeater {
        model: pill.layers

        Rectangle {
            x: -modelData.grow
            y: pill.drop - modelData.grow
            width: pill.width + modelData.grow * 2
            height: pill.height + modelData.grow * 2
            radius: Math.max(0, pill.radius + modelData.grow)
            color: Qt.rgba(0, 0, 0, modelData.alpha)
        }
    }

    Rectangle {
        anchors.fill: parent
        radius: pill.radius
        color: pill.fill
        border.width: 1
        border.color: pill.rim
    }
}
