use super::*;
use std::collections::BTreeMap;

#[test]
fn native_whisper_passes_auto_and_explicit_languages_to_the_executable() {
    for language in [None, Some("auto"), Some("fa")] {
        let mut values = BTreeMap::from([
            (
                "TELEGRAM_TRANSCRIBE_ENGINE".to_owned(),
                "whisper".to_owned(),
            ),
            (
                "TELEGRAM_TRANSCRIBE_WHISPER_CLI".to_owned(),
                "fixture-native-executable".to_owned(),
            ),
            (
                "TELEGRAM_TRANSCRIBE_WHISPER_MODEL_PATH".to_owned(),
                "fixture-model.bin".to_owned(),
            ),
        ]);
        if let Some(language) = language {
            values.insert(
                "TELEGRAM_TRANSCRIBE_LANGUAGE".to_owned(),
                language.to_owned(),
            );
        }
        let settings = Settings::from_values(&values, &json!({})).unwrap();
        let command = command(
            &settings,
            Path::new("recording.wav"),
            Path::new("transcript"),
        )
        .unwrap();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        let position = args.iter().position(|arg| arg == "-l").unwrap();
        assert_eq!(args[position + 1], language.unwrap_or("auto"));
        assert_eq!(args.iter().filter(|arg| *arg == "-l").count(), 1);
    }
}

#[test]
fn ffmpeg_decoder_and_filter_threads_are_bounded_before_the_input() {
    let mut command = media::decoder();
    command.args(["-i", "fixture.webm", "-threads", "1", "fixture.png"]);
    let args: Vec<_> = command
        .as_std()
        .get_args()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    let input = args.iter().position(|arg| arg == "-i").unwrap();
    for option in ["-threads", "-filter_threads", "-filter_complex_threads"] {
        let index = args.iter().position(|arg| arg == option).unwrap();
        assert!(index < input);
        assert_eq!(args[index + 1], "1");
    }
}
