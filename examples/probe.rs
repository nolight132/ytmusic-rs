use std::path::PathBuf;

use ytmusic::YtMusic;

fn token_path() -> PathBuf {
    std::env::var_os("YTMUSIC_TOKENS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/ytmusic-tokens.json"))
}

fn client() -> YtMusic {
    if let Some(path) = std::env::var_os("YTMUSIC_COOKIES")
        && let Ok(cookies) = std::fs::read_to_string(path)
    {
        eprintln!("probe: using cookie auth");
        return YtMusic::with_cookies(cookies.trim());
    }
    match ytmusic::Tokens::load(&token_path()) {
        Ok(Some(tokens)) => {
            eprintln!("probe: using tokens from {}", token_path().display());
            YtMusic::new(tokens).persist_to(token_path())
        }
        _ => {
            eprintln!("probe: anonymous session");
            YtMusic::anonymous()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let command = std::env::args().nth(1).unwrap_or_default();
    let argument = std::env::args().nth(2).unwrap_or_default();
    let api = client();
    match command.as_str() {
        "login" => {
            let http = reqwest::Client::new();
            let identity = ytmusic::oauth::fetch_identity(&http).await;
            let device = ytmusic::oauth::request_device_code(&http, &identity).await?;
            println!(
                "visit {} and enter code {}",
                device.verification_url, device.user_code
            );
            let tokens = ytmusic::oauth::poll_token(&http, &identity, &device).await?;
            tokens.save(&token_path())?;
            println!("tokens saved to {}", token_path().display());
        }
        "search" => {
            for track in api.search_songs(&argument).await? {
                println!(
                    "{:14} {:40} {:24} [{}] {:?}",
                    track.video_id.unwrap_or_default(),
                    truncate(&track.title, 40),
                    truncate(
                        &track
                            .artists
                            .iter()
                            .map(|a| a.name.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                        24
                    ),
                    track
                        .album
                        .as_ref()
                        .map(|album| {
                            format!("{} {}", album.name, album.id.clone().unwrap_or_default())
                        })
                        .unwrap_or_else(|| "-".to_string()),
                    track.duration,
                );
            }
        }
        "album" => {
            let detail = api.album(&argument).await?;
            println!(
                "{} by {:?} ({:?}, {:?}) tracks={} playlist={:?}",
                detail.album.title,
                detail.album.artists,
                detail.album.kind,
                detail.album.year,
                detail.tracks.len(),
                detail.album.playlist_id,
            );
            for track in &detail.tracks {
                println!(
                    "  {:14} {:40} {:?}",
                    track.video_id.clone().unwrap_or_default(),
                    truncate(&track.title, 40),
                    track.duration
                );
            }
        }
        "artist" => {
            let artist = api.artist(&argument).await?;
            println!(
                "{} subs={:?} top={} albums={} singles={} thumbs={}",
                artist.name,
                artist.subscribers,
                artist.top_tracks.len(),
                artist.albums.len(),
                artist.singles.len(),
                artist.thumbnails.len(),
            );
            for album in artist.albums.iter().chain(artist.singles.iter()) {
                println!(
                    "  {:20} {:?} {:?}",
                    album.browse_id, album.title, album.year
                );
            }
        }
        "playlist" => {
            let detail = api.playlist(&argument).await?;
            println!(
                "{} by {:?} owned={} public={} tracks={}",
                detail.playlist.title,
                detail.playlist.author,
                detail.playlist.owned,
                detail.public,
                detail.tracks.len(),
            );
            for track in detail.tracks.iter().take(10) {
                println!(
                    "  {:14} {}",
                    track.video_id.clone().unwrap_or_default(),
                    truncate(&track.title, 60)
                );
            }
        }
        "radio" => {
            for track in api.track_radio(&argument).await? {
                println!(
                    "{:14} {}",
                    track.video_id.unwrap_or_default(),
                    truncate(&track.title, 60)
                );
            }
        }
        "stream" => {
            let format = api.best_audio(&argument).await?;
            println!(
                "itag={} codec={} bitrate={} length={:?} loudness={:?}",
                format.itag,
                format.codec,
                format.bitrate,
                format.content_length,
                format.loudness_db
            );
            println!("{}", format.url);
        }
        "liked" => {
            for track in api.liked_songs().await? {
                println!(
                    "{:14} {}",
                    track.video_id.unwrap_or_default(),
                    truncate(&track.title, 60)
                );
            }
        }
        "albums" => {
            for album in api.library_albums().await? {
                println!("{:20} {}", album.browse_id, album.title);
            }
        }
        "playlists" => {
            for playlist in api.library_playlists().await? {
                println!("{:40} {}", playlist.id, playlist.title);
            }
        }
        "matrix" => {
            let clients = [
                ("music", ytmusic::Client::Music),
                ("androidmusic", ytmusic::Client::AndroidMusic),
                ("tv", ytmusic::Client::Tv),
                ("ios", ytmusic::Client::Ios),
            ];
            for (label, client) in clients {
                let search = api
                    .execute(
                        "search",
                        client,
                        serde_json::json!({ "query": "daft punk" }),
                    )
                    .await
                    .map(|_| "OK")
                    .unwrap_or("FAIL");
                let album = api
                    .execute(
                        "browse",
                        client,
                        serde_json::json!({ "browseId": "MPREb_K8qWMWVqXGi" }),
                    )
                    .await
                    .map(|_| "OK")
                    .unwrap_or("FAIL");
                let liked = api
                    .execute(
                        "browse",
                        client,
                        serde_json::json!({ "browseId": "FEmusic_liked_playlists" }),
                    )
                    .await
                    .map(|_| "OK")
                    .unwrap_or("FAIL");
                println!("{label:14} search={search} album={album} library={liked}");
            }
        }
        "authcheck" => {
            for (label, client) in [
                ("tv", ytmusic::Client::Tv),
                ("androidvr", ytmusic::Client::AndroidVr),
                ("ios", ytmusic::Client::Ios),
            ] {
                let player = match api.player_response("4D7u5KF7SP8", client).await {
                    Ok(response) => {
                        let status = response["playabilityStatus"]["status"]
                            .as_str()
                            .unwrap_or("?")
                            .to_string();
                        let direct = response["streamingData"]["adaptiveFormats"]
                            .as_array()
                            .map(|formats| {
                                formats.iter().filter(|f| f.get("url").is_some()).count()
                            })
                            .unwrap_or(0);
                        format!("{status}, {direct} direct formats")
                    }
                    Err(error) => format!("ERR {error}"),
                };
                println!("player via {label}: {player}");
            }
            let created = api
                .execute(
                    "playlist/create",
                    ytmusic::Client::Tv,
                    serde_json::json!({ "title": "sonora probe", "privacyStatus": "PRIVATE" }),
                )
                .await;
            match created {
                Ok(response) => {
                    let id = response["playlistId"].as_str().unwrap_or("").to_string();
                    println!("playlist/create via tv: OK {id}");
                    let deleted = api
                        .execute(
                            "playlist/delete",
                            ytmusic::Client::Tv,
                            serde_json::json!({ "playlistId": id }),
                        )
                        .await;
                    println!(
                        "playlist/delete via tv: {}",
                        match deleted {
                            Ok(_) => "OK".to_string(),
                            Err(error) => format!("ERR {error}"),
                        }
                    );
                }
                Err(error) => println!("playlist/create via tv: ERR {error}"),
            }
            let albums = api
                .execute(
                    "browse",
                    ytmusic::Client::Tv,
                    serde_json::json!({ "browseId": "FEmusic_liked_albums" }),
                )
                .await
                .map(|response| response.to_string().len())
                .map(|size| format!("OK {size} bytes"))
                .unwrap_or_else(|error| format!("ERR {error}"));
            println!("browse liked albums via tv: {albums}");
        }
        "rawbrowse" => {
            let client = match std::env::args().nth(3).as_deref() {
                Some("tv") => ytmusic::Client::Tv,
                Some("androidmusic") => ytmusic::Client::AndroidMusic,
                Some("ios") => ytmusic::Client::Ios,
                _ => ytmusic::Client::Music,
            };
            let response = api
                .execute(
                    "browse",
                    client,
                    serde_json::json!({ "browseId": argument }),
                )
                .await?;
            let text = response.to_string();
            println!("{}", &text[..text.len().min(2000)]);
        }
        "profile" => {
            let profile = api.profile().await?;
            println!("{} ({:?})", profile.name, profile.email);
        }
        other => {
            eprintln!("unknown command: {other}");
            eprintln!(
                "usage: probe <login|search|album|artist|playlist|radio|stream|liked|albums|playlists|profile> [arg]"
            );
        }
    }
    Ok(())
}

fn truncate(text: &str, max: usize) -> String {
    match text.chars().count() > max {
        true => text.chars().take(max - 1).collect::<String>() + "…",
        false => text.to_string(),
    }
}
