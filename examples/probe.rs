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
        "rmplaylist" => {
            api.delete_playlist(&argument).await?;
            println!("deleted {argument}");
        }
        "suite" => {
            let mut results: Vec<(&str, String)> = Vec::new();
            let pause = || tokio::time::sleep(std::time::Duration::from_secs(3));

            let search = api.search_songs("daft punk get lucky").await;
            let track_id = search
                .as_ref()
                .ok()
                .and_then(|tracks| tracks.first())
                .and_then(|track| track.video_id.clone());
            results.push(("search", verdict(search.as_ref().map(Vec::len))));
            pause().await;

            let album = api.album("MPREb_K8qWMWVqXGi").await;
            results.push((
                "album",
                verdict(album.as_ref().map(|detail| detail.tracks.len())),
            ));
            pause().await;

            let artist = api.artist("UCRr1xG_2WIDs18a6cIiCxeA").await;
            results.push((
                "artist",
                verdict(artist.as_ref().map(|artist| artist.top_tracks.len())),
            ));
            pause().await;

            let radio = api.track_radio("4D7u5KF7SP8").await;
            results.push(("radio", verdict(radio.as_ref().map(Vec::len))));
            pause().await;

            let stream = api.best_audio("4D7u5KF7SP8").await;
            match &stream {
                Ok(format) => {
                    let data = api.download(format).await;
                    results.push(("stream+download", verdict(data.as_ref().map(Vec::len))));
                }
                Err(error) => results.push(("stream+download", format!("FAIL {error:#}"))),
            }
            pause().await;

            let profile = api.profile().await;
            results.push((
                "profile",
                verdict(profile.as_ref().map(|profile| profile.name.len())),
            ));
            pause().await;

            let liked = api.liked_songs().await;
            results.push(("liked songs", verdict(liked.as_ref().map(Vec::len))));
            pause().await;

            let albums = api.library_albums().await;
            results.push(("library albums", verdict(albums.as_ref().map(Vec::len))));
            pause().await;

            let playlists = api.library_playlists().await;
            results.push((
                "library playlists",
                verdict(playlists.as_ref().map(Vec::len)),
            ));
            pause().await;

            match api.create_playlist("sonora test suite").await {
                Ok(playlist_id) => {
                    results.push(("playlist create", format!("OK {playlist_id}")));
                    pause().await;
                    let rename = api
                        .rename_playlist(&playlist_id, "sonora test renamed")
                        .await;
                    results.push(("playlist rename", verdict(rename.as_ref().map(|_| 0))));
                    pause().await;
                    if let Some(track_id) = &track_id {
                        let add = api.add_playlist_track(&playlist_id, track_id).await;
                        results.push(("playlist add track", verdict(add.as_ref().map(|_| 0))));
                        pause().await;
                        let detail = api.playlist(&playlist_id).await;
                        let set_video_id = detail
                            .as_ref()
                            .ok()
                            .and_then(|detail| detail.tracks.first())
                            .and_then(|track| track.set_video_id.clone());
                        results.push((
                            "playlist read back",
                            verdict(detail.as_ref().map(|detail| detail.tracks.len())),
                        ));
                        pause().await;
                        match set_video_id {
                            Some(set_video_id) => {
                                let removed = api
                                    .remove_playlist_track(&playlist_id, track_id, &set_video_id)
                                    .await;
                                results.push((
                                    "playlist remove track",
                                    verdict(removed.as_ref().map(|_| 0)),
                                ));
                            }
                            None => results
                                .push(("playlist remove track", "SKIP no setVideoId".to_string())),
                        }
                        pause().await;
                        let like = api.rate_track(track_id, true).await;
                        results.push(("like track", verdict(like.as_ref().map(|_| 0))));
                        pause().await;
                        let unlike = api.rate_track(track_id, false).await;
                        results.push(("unlike track", verdict(unlike.as_ref().map(|_| 0))));
                        pause().await;
                    }
                    let deleted = api.delete_playlist(&playlist_id).await;
                    results.push(("playlist delete", verdict(deleted.as_ref().map(|_| 0))));
                }
                Err(error) => results.push(("playlist create", format!("FAIL {error:#}"))),
            }

            println!();
            for (name, result) in &results {
                println!("{name:22} {result}");
            }
            let failed = results
                .iter()
                .filter(|(_, r)| r.starts_with("FAIL"))
                .count();
            println!("\n{} checks, {} failed", results.len(), failed);
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

fn verdict<T: std::fmt::Display, E: std::fmt::Display>(result: Result<T, &E>) -> String {
    match result {
        Ok(value) => format!("OK {value}"),
        Err(error) => format!("FAIL {error}"),
    }
}

fn truncate(text: &str, max: usize) -> String {
    match text.chars().count() > max {
        true => text.chars().take(max - 1).collect::<String>() + "…",
        false => text.to_string(),
    }
}
