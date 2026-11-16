use crate::target::{AudioApp, MicrophoneDevice};

const PLAYBACK_STREAM: &str = "Stream/Output/Audio";
const MICROPHONE: &str = "Audio/Source";
const VIRTUAL_MICROPHONE: &str = "Audio/Source/Virtual";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Node {
    pub(super) id: u32,
    pub(super) properties: Vec<(String, String)>,
}

impl Node {
    fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    fn plays_sound(&self) -> bool {
        self.property("media.class") == Some(PLAYBACK_STREAM)
    }

    fn process_id(&self) -> Option<u64> {
        self.property("application.process.id")?.parse().ok()
    }
}

pub(super) fn audio_apps(nodes: &[Node], own_process_id: u32) -> Vec<AudioApp> {
    nodes
        .iter()
        .filter(|node| node.plays_sound() && node.process_id() != Some(u64::from(own_process_id)))
        .map(|node| {
            let name = node
                .property("application.name")
                .or(node.property("node.name"))
                .unwrap_or("Unnamed app");
            AudioApp::new(u64::from(node.id), name)
        })
        .collect()
}

pub(super) fn playback_node_of(nodes: &[Node], app: &AudioApp) -> Option<u32> {
    let playing = || nodes.iter().filter(|node| node.plays_sound());
    playing()
        .find(|node| u64::from(node.id) == app.id)
        .or_else(|| playing().find(|node| node.process_id() == Some(app.id)))
        .map(|node| node.id)
}

pub(super) fn microphones(nodes: &[Node], default_name: Option<&str>) -> Vec<MicrophoneDevice> {
    nodes
        .iter()
        .filter(|node| {
            matches!(
                node.property("media.class"),
                Some(MICROPHONE | VIRTUAL_MICROPHONE)
            )
        })
        .filter_map(|node| {
            let id = node.property("node.name")?;
            let name = node
                .property("node.description")
                .or(node.property("node.nick"))
                .unwrap_or(id);
            Some(MicrophoneDevice::new(id, name, default_name == Some(id)))
        })
        .collect()
}

pub(super) fn default_node_name(metadata_value: &str) -> Option<String> {
    let after_key = metadata_value.split_once("\"name\"")?.1;
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let quoted = after_colon.trim_start().strip_prefix('"')?;
    let name = quoted.split_once('"')?.0;
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u32, properties: &[(&str, &str)]) -> Node {
        Node {
            id,
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    fn graph() -> Vec<Node> {
        vec![
            node(
                40,
                &[
                    ("media.class", "Audio/Sink"),
                    ("node.name", "alsa_output.speakers"),
                ],
            ),
            node(
                51,
                &[
                    ("media.class", PLAYBACK_STREAM),
                    ("application.name", "Firefox"),
                    ("application.process.id", "900"),
                ],
            ),
            node(
                52,
                &[("media.class", PLAYBACK_STREAM), ("node.name", "mpv")],
            ),
            node(
                53,
                &[
                    ("media.class", PLAYBACK_STREAM),
                    ("application.name", "ngine"),
                    ("application.process.id", "77"),
                ],
            ),
            node(
                60,
                &[
                    ("media.class", MICROPHONE),
                    ("node.name", "alsa_input.usb"),
                    ("node.description", "USB Microphone"),
                ],
            ),
            node(
                61,
                &[
                    ("media.class", VIRTUAL_MICROPHONE),
                    ("node.name", "denoised"),
                ],
            ),
            node(62, &[("media.class", MICROPHONE)]),
        ]
    }

    #[test]
    fn the_apps_playing_sound_are_the_playback_streams_of_other_processes() {
        assert_eq!(
            audio_apps(&graph(), 77),
            [AudioApp::new(51, "Firefox"), AudioApp::new(52, "mpv")]
        );
    }

    #[test]
    fn an_app_is_found_by_its_stream_and_otherwise_by_its_process() {
        let nodes = graph();

        assert_eq!(
            playback_node_of(&nodes, &AudioApp::new(52, "mpv")),
            Some(52)
        );
        assert_eq!(
            playback_node_of(&nodes, &AudioApp::new(900, "Firefox")),
            Some(51)
        );
        assert_eq!(playback_node_of(&nodes, &AudioApp::new(40, "Sink")), None);
        assert_eq!(playback_node_of(&nodes, &AudioApp::new(1, "Gone")), None);
    }

    #[test]
    fn microphones_are_the_named_sources_and_one_is_the_default() {
        assert_eq!(
            microphones(&graph(), Some("denoised")),
            [
                MicrophoneDevice::new("alsa_input.usb", "USB Microphone", false),
                MicrophoneDevice::new("denoised", "denoised", true),
            ]
        );
        assert!(
            microphones(&graph(), None)
                .iter()
                .all(|device| !device.is_default)
        );
    }

    #[test]
    fn the_default_device_is_read_from_the_metadata_value() {
        assert_eq!(
            default_node_name(r#"{"name":"alsa_input.usb"}"#).as_deref(),
            Some("alsa_input.usb")
        );
        assert_eq!(
            default_node_name(r#"{ "name" : "a b" }"#).as_deref(),
            Some("a b")
        );
        assert_eq!(default_node_name(r#"{"name":""}"#), None);
        assert_eq!(default_node_name(r#"{"nick":"x"}"#), None);
        assert_eq!(default_node_name("alsa_input.usb"), None);
    }
}
