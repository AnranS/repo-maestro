use std::path::Path;

use crate::config::channels::{parse_channels_yaml, ChannelConfig};

pub fn load_channels_config(workspace_root: &Path) -> Option<ChannelConfig> {
    let path = workspace_root.join(".maestro").join("channels.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("channels config not found at {}", path.display());
            return None;
        }
        Err(source) => {
            tracing::warn!(
                "could not read channels config {}: {source}",
                path.display()
            );
            return None;
        }
    };

    match parse_channels_yaml(&text) {
        Ok(config) => Some(config),
        Err(err) => {
            tracing::warn!(
                "could not parse channels config {}: {err:?}",
                path.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[test]
    fn load_channels_config_returns_none_when_file_missing() {
        let temp = tempfile::tempdir().unwrap();

        assert_eq!(super::load_channels_config(temp.path()), None);
    }

    #[test]
    fn load_channels_config_returns_none_when_file_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join(".maestro");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("channels.yaml"), "version: nope\n").unwrap();

        assert_eq!(super::load_channels_config(temp.path()), None);
    }

    #[test]
    fn load_channels_config_returns_some_when_file_valid() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join(".maestro");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("channels.yaml"),
            r#"
version: 1
channels:
  feishu:
    enabled: true
"#,
        )
        .unwrap();

        let config = super::load_channels_config(temp.path()).expect("valid config");

        assert!(config.channels["feishu"].enabled);
    }

    #[test]
    fn load_channels_config_missing_file_is_quiet_at_info_level() {
        let temp = tempfile::tempdir().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let writer = CapturedWriter {
            captured: captured.clone(),
        };
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(super::load_channels_config(temp.path()), None);
        });

        let output = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(
            !output.contains("channels config not found"),
            "missing optional channels.yaml should not print at INFO: {output}"
        );
    }

    #[derive(Clone)]
    struct CapturedWriter {
        captured: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.captured.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
