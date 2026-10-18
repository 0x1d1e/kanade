import QtQuick 2.15

// one line of text, set by its baseline, with the metrics of its font to place others by
Text {
    id: line

    property real size: 14
    property int weight: Font.Normal
    property string face: "sans-serif"
    property color ink: "#f2f2f7"

    // where the baseline falls, in the parent's coordinates
    property real base: 0

    // the widest the line gets before it is cut with an ellipsis
    property real maxWidth: 100000

    // the font's ascender and descender, and the height of its capitals
    readonly property real ascent: metrics.ascent
    readonly property real descent: metrics.descent
    readonly property real cap: capital.tightBoundingRect.height

    color: ink
    font.family: face
    font.pixelSize: size
    font.weight: weight
    textFormat: Text.PlainText
    wrapMode: Text.NoWrap
    elide: Text.ElideRight
    width: Math.min(implicitWidth, maxWidth)
    y: base - baselineOffset

    FontMetrics {
        id: metrics
        font.family: line.face
        font.pixelSize: line.size
        font.weight: line.weight
    }

    TextMetrics {
        id: capital
        font.family: line.face
        font.pixelSize: line.size
        font.weight: line.weight
        text: "H"
    }
}
