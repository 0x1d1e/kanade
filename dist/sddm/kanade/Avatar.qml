import QtQuick 2.15

// the user's picture cut round, else their initial on a black disc. The picture is clipped by a
// canvas, which any renderer draws; if it cannot be read the disc and initial stay
Item {
    id: avatar

    // image urls to try in order; the first that loads is drawn
    property var sources: []
    property string initial: ""
    property string face: "sans-serif"
    property color ink: "#f2f2f7"

    // the first source whose image has loaded, or -1
    readonly property int chosen: first.status === Image.Ready ? 0 : (second.status === Image.Ready ? 1 : -1)

    width: 72
    height: 72

    Pill {
        visible: avatar.chosen < 0
        anchors.fill: parent
        radius: width / 2
    }

    Line {
        visible: avatar.chosen < 0
        text: avatar.initial
        face: avatar.face
        ink: avatar.ink
        size: avatar.height * 0.42
        weight: Font.DemiBold
        x: (avatar.width - width) / 2
        base: avatar.height / 2 + cap / 2
    }

    // read for status and pixels only, never shown
    Image {
        id: first
        visible: false
        asynchronous: true
        source: avatar.sources.length > 0 ? avatar.sources[0] : ""
        onStatusChanged: canvas.requestPaint()
    }

    Image {
        id: second
        visible: false
        asynchronous: true
        source: avatar.sources.length > 1 ? avatar.sources[1] : ""
        onStatusChanged: canvas.requestPaint()
    }

    Canvas {
        id: canvas
        // drawn at four times the size and shown at a quarter, so the cut edge is smooth
        width: avatar.width * 4
        height: avatar.height * 4
        scale: 0.25
        transformOrigin: Item.TopLeft
        visible: avatar.chosen >= 0
        renderTarget: Canvas.Image
        canvasSize: Qt.size(width, height)
        smooth: true

        onPaint: {
            var ctx = getContext("2d")
            var size = avatar.width * 4
            ctx.reset()
            ctx.clearRect(0, 0, size, size)
            var source = avatar.chosen === 0 ? first : (avatar.chosen === 1 ? second : null)
            if (source === null)
                return
            var w = source.implicitWidth
            var h = source.implicitHeight
            if (w <= 0 || h <= 0)
                return
            // the picture covers the disc, cropped to its middle
            var cover = size / Math.min(w, h)
            ctx.save()
            ctx.beginPath()
            ctx.arc(size / 2, size / 2, size / 2, 0, Math.PI * 2, true)
            ctx.closePath()
            ctx.clip()
            ctx.drawImage(source, (size - w * cover) / 2, (size - h * cover) / 2, w * cover, h * cover)
            ctx.restore()
        }

        onVisibleChanged: requestPaint()
    }

    onChosenChanged: canvas.requestPaint()
}
