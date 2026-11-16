import QtQuick
import Quickshell.Io
import qs.Commons
import qs.Ui

// The Omarchy 4 bar widget for ngine. It reads the same JSON lines as the
// Waybar module from `ngine status --waybar --watch`, which blocks while
// nothing changes and reconnects by itself when ngine starts again.
BarWidget {
  id: root
  moduleName: "ngine.status"

  property var state: ({ text: "", tooltip: "", alt: "stopped" })
  readonly property bool recording: state.alt === "recording"

  visible: state.text !== ""
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Process {
    id: watch
    command: ["ngine", "status", "--waybar", "--watch"]
    running: true
    stdout: SplitParser {
      onRead: function(line) {
        try { root.state = JSON.parse(line) } catch (e) {}
      }
    }
    onExited: root.state = ({ text: "", tooltip: "", alt: "stopped" })
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.recording ? "\u{f0ec2}" : "\u{f02da}"
    labelText: root.recording ? String(root.state.text).split(" ").slice(1).join(" ") : ""
    slotSize: Style.bar.statusSlot
    tooltipText: root.state.tooltip
    onPressed: {
      if (root.bar) root.bar.run("ngine record stop || ngine replay save")
    }
  }
}
