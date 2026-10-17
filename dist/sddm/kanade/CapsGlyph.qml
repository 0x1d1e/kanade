import QtQuick 2.15
import QtQuick.Shapes 1.15

// Caps Lock: an arrow up over a bar, in a square `size` across
Shape {
    id: glyph

    property real size: 16
    property color ink: "#c2c4c7"

    width: size
    height: size

    ShapePath {
        fillColor: glyph.ink
        strokeColor: "transparent"
        strokeWidth: 0
        startX: glyph.size * 0.5
        startY: 0
        PathLine { x: glyph.size; y: glyph.size * 0.5 }
        PathLine { x: glyph.size * 0.72; y: glyph.size * 0.5 }
        PathLine { x: glyph.size * 0.72; y: glyph.size * 0.72 }
        PathLine { x: glyph.size * 0.28; y: glyph.size * 0.72 }
        PathLine { x: glyph.size * 0.28; y: glyph.size * 0.5 }
        PathLine { x: 0; y: glyph.size * 0.5 }
        PathLine { x: glyph.size * 0.5; y: 0 }
    }

    ShapePath {
        fillColor: glyph.ink
        strokeColor: "transparent"
        strokeWidth: 0
        startX: glyph.size * 0.28
        startY: glyph.size * 0.84
        PathLine { x: glyph.size * 0.72; y: glyph.size * 0.84 }
        PathLine { x: glyph.size * 0.72; y: glyph.size * 0.98 }
        PathLine { x: glyph.size * 0.28; y: glyph.size * 0.98 }
        PathLine { x: glyph.size * 0.28; y: glyph.size * 0.84 }
    }
}
