import QtQuick 2.15
import QtQuick.Shapes 1.15

/*
 * a line glyph: one SVG path on a 24x24 view box, stroked in `ink` and `size` across, no fill. The
 * icons beside the chips; a name it does not know draws nothing.
 */
Item {
    id: glyph

    property string name: ""
    property real size: 14
    property color ink: "#c2c4c7"

    implicitWidth: size
    implicitHeight: size

    function path() {
        if (name === "user")
            return "M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2 M12 3a4 4 0 1 1 0 8 4 4 0 1 1 0-8z"
        if (name === "session")
            return "M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z M8 21h8 M12 17v4"
        if (name === "keyboard")
            return "M4 5h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z M7 9h2 M12 9h2 M17 9h2 M8 13h8"
        if (name === "sleep")
            return "M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"
        if (name === "restart")
            return "M23 4v6h-6 M20.49 15a9 9 0 1 1-2.12-9.36L23 10"
        if (name === "power")
            return "M18.36 6.64a9 9 0 1 1-12.73 0 M12 2v10"
        return ""
    }

    Shape {
        anchors.fill: parent

        ShapePath {
            // the path is read on its 24x24 view box and `scale` fits it to `size`; the stroke is
            // not scaled with it, so it is read in the pixels the glyph is drawn in
            scale: Qt.size(glyph.size / 24, glyph.size / 24)
            strokeColor: glyph.ink
            strokeWidth: glyph.size * 1.6 / 24
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            fillColor: "transparent"

            PathSvg { path: glyph.path() }
        }
    }
}
