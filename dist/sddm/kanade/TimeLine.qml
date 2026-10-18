import QtQuick 2.15

// the time, its digits each in a cell as wide as the widest, so it does not shift as the minutes
// change (the font's own tabular figures are a Qt 6.7 feature; this runs on Qt 5.15 too)
Item {
    id: time

    property string text: ""
    property string face: "sans-serif"
    property real size: 112
    property int weight: Font.DemiBold
    property color ink: "#f2f2f7"

    // the baseline, in the parent's coordinates
    property real base: 0

    // the font's capital height, to place the baseline by
    readonly property real cap: capital.tightBoundingRect.height
    readonly property real ascent: metrics.ascent

    // set by `remeasure`, not bound: measuring writes the ruler it reads
    property var glyphs: []

    width: row.width
    height: metrics.ascent + metrics.descent
    y: base - metrics.ascent

    function remeasure() {
        var cell = 0
        for (var digit = 0; digit < 10; digit++) {
            ruler.text = String(digit)
            cell = Math.max(cell, ruler.advanceWidth)
        }
        var out = []
        for (var i = 0; i < text.length; i++) {
            var ch = text.charAt(i)
            ruler.text = ch
            var digitOnly = ch >= "0" && ch <= "9"
            out.push({ ch: ch, width: digitOnly ? cell : ruler.advanceWidth })
        }
        glyphs = out
    }

    onTextChanged: remeasure()
    onFaceChanged: remeasure()
    onSizeChanged: remeasure()
    onWeightChanged: remeasure()
    Component.onCompleted: remeasure()

    FontMetrics {
        id: metrics
        font.family: time.face
        font.pixelSize: time.size
        font.weight: time.weight
    }

    TextMetrics {
        id: capital
        font.family: time.face
        font.pixelSize: time.size
        font.weight: time.weight
        text: "H"
    }

    TextMetrics {
        id: ruler
        font.family: time.face
        font.pixelSize: time.size
        font.weight: time.weight
    }

    Row {
        id: row
        spacing: 0

        Repeater {
            model: time.glyphs

            Text {
                width: modelData.width
                height: time.height
                text: modelData.ch
                color: time.ink
                font.family: time.face
                font.pixelSize: time.size
                font.weight: time.weight
                textFormat: Text.PlainText
                horizontalAlignment: Text.AlignHCenter
            }
        }
    }
}
