pragma Singleton
import QtQuick
QtObject {
    property int gridUnit: 18
    property int smallSpacing: 4
    property int largeSpacing: 8
    property int cornerRadius: 5
    property QtObject iconSizes: QtObject { property int small: 16; property int large: 48 }
}
