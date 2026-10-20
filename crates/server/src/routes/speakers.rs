use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use locust_core::database::EntryFilter;
use locust_core::speakers::{group_speakers, SpeakerGroups, SpeakerNotes, NOTES_METADATA_KEY};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{err, try_project_operation, ApiError, AppState, ProjectInfo};

/// Clients echo the exact project identity returned by project/current or open.
#[derive(Deserialize)]
pub(crate) struct SpeakerProject {
    database_path: PathBuf,
    project_path: PathBuf,
    format_id: String,
}

async fn check_owner(state: &AppState, owner: &SpeakerProject) -> Result<ProjectInfo, ApiError> {
    let project = state
        .current_project
        .read()
        .await
        .clone()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "no project open"))?;
    if project.database_path.as_ref() != Some(&owner.database_path)
        || state.db.path() != owner.database_path
        || project.path != owner.project_path
        || project.format_id != owner.format_id
    {
        return Err(err(
            StatusCode::CONFLICT,
            "speaker project changed; reload the project",
        ));
    }
    Ok(project)
}

fn read_notes(state: &AppState) -> Result<SpeakerNotes, ApiError> {
    state
        .db
        .get_project_metadata(NOTES_METADATA_KEY)
        .and_then(|value| {
            value
                .map(serde_json::from_value)
                .transpose()
                .map_err(Into::into)
        })
        .map(|notes| notes.unwrap_or_default())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

fn groups(state: &AppState, project: &ProjectInfo) -> Result<SpeakerGroups, ApiError> {
    let entries = state
        .db
        .get_entries(&EntryFilter::default())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(group_speakers(
        &entries,
        &project.format_id,
        &read_notes(state)?,
    ))
}

pub(crate) async fn list_speakers(
    State(state): State<Arc<AppState>>,
    Query(owner): Query<SpeakerProject>,
) -> Result<Json<SpeakerGroups>, ApiError> {
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    let project = check_owner(&state, &owner).await?;
    groups(&state, &project).map(Json)
}

#[derive(Deserialize)]
pub(crate) struct SpeakerNoteRequest {
    #[serde(flatten)]
    owner: SpeakerProject,
    speaker_id: String,
    note: String,
}

pub(crate) async fn save_speaker_note(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SpeakerNoteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Same admission as project open: keep ownership across metadata read/modify/
    // write. Each helper uses the live DB mutex; no await follows the owner check.
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    let project = check_owner(&state, &req.owner).await?;
    if !groups(&state, &project)?
        .groups
        .iter()
        .any(|group| group.id == req.speaker_id)
    {
        return Err(err(StatusCode::NOT_FOUND, "speaker not found"));
    }
    let mut notes = read_notes(&state)?;
    if req.note.is_empty() {
        notes.remove(&req.speaker_id);
    } else {
        notes.insert(req.speaker_id.clone(), req.note.clone());
    }
    state
        .db
        .set_project_metadata(
            NOTES_METADATA_KEY,
            &serde_json::to_value(notes).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?,
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(
        serde_json::json!({"speaker_id": req.speaker_id, "note": req.note}),
    ))
}

#[cfg(test)]
mod tests {
    use crate::routes::test_support::*;
    use crate::ProjectExclusiveGuard;
    use locust_core::database::Database;
    use locust_core::models::StringEntry;
    use locust_core::speakers::NOTES_METADATA_KEY;
    use serde_json::{json, Value};

    async fn response(request: reqwest::RequestBuilder, expected: u16) -> Value {
        let response = request.send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, expected, "{body}");
        serde_json::from_str(&body).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn speakers_api_groups_persists_and_rejects_stale_ownership() {
        let (url, handle, state) = setup_with_state().await;
        let client = client();
        let endpoint = format!("{url}/api/speakers");
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(game.join("game")).unwrap();
        std::fs::write(game.join("game/script.rpy"), "label start:\n    alice \"Hello\"\n    alice \"Again\"\n    bob \"Hello\"\n    \"Narration\"\n").unwrap();
        let db_path = dir.path().join("game.locust.db");
        let owner = json!({"database_path": db_path, "project_path": game, "format_id": "renpy"});
        response(client.get(&endpoint).query(&owner), 400).await;
        response(
            client
                .post(format!("{url}/api/project/open"))
                .json(&json!({"path": game, "format_id": "renpy"})),
            200,
        )
        .await;
        let before = response(client.get(format!("{url}/api/strings")), 200).await;
        assert_eq!(before["total"], 4);
        state
            .db
            .set_project_metadata("unrelated", &json!({"keep": true}))
            .unwrap();
        let listed = response(client.get(&endpoint).query(&owner), 200).await;
        assert_eq!(listed["groups"].as_array().unwrap().len(), 2);
        assert_eq!(listed["unassigned_entry_ids"].as_array().unwrap().len(), 1);
        let alice = listed["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["name"] == "alice")
            .unwrap();
        assert_eq!(alice["count"], 2);
        assert_eq!(alice["entry_ids"].as_array().unwrap().len(), 2);
        for id in alice["entry_ids"].as_array().unwrap() {
            assert!(before["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == *id));
        }
        let mut write = owner.clone();
        write["speaker_id"] = alice["id"].clone();
        write["note"] = json!("Uses formal speech.");
        let note_url = format!("{endpoint}/note");
        response(client.put(&note_url).json(&write), 200).await;
        let saved = state.db.get_project_metadata(NOTES_METADATA_KEY).unwrap();
        let guard = ProjectExclusiveGuard::try_enter(&state.project_exclusive).unwrap();
        write["note"] = json!("must not write");
        response(client.put(&note_url).json(&write), 409).await;
        drop(guard);
        for (field, value) in [
            ("project_path", json!(dir.path().join("wrong-game"))),
            ("database_path", json!(dir.path().join("wrong.db"))),
            ("format_id", json!("html")),
        ] {
            let mut stale = write.clone();
            stale[field] = value;
            response(client.put(&note_url).json(&stale), 409).await;
        }
        assert_eq!(
            state.db.get_project_metadata(NOTES_METADATA_KEY).unwrap(),
            saved
        );
        // Actual project switch, not just a forged request field.
        let other = dir.path().join("other");
        std::fs::create_dir_all(other.join("game")).unwrap();
        std::fs::write(
            other.join("game/script.rpy"),
            "label start:\n    alice \"Other\"\n",
        )
        .unwrap();
        response(
            client
                .post(format!("{url}/api/project/open"))
                .json(&json!({"path": other, "format_id": "renpy"})),
            200,
        )
        .await;
        response(client.put(&note_url).json(&write), 409).await;
        response(client.get(&endpoint).query(&owner), 409).await;
        assert_eq!(
            state.db.get_project_metadata(NOTES_METADATA_KEY).unwrap(),
            None
        );
        // Reopen saved DB through the existing public API.
        response(
            client
                .post(format!("{url}/api/project/open-db"))
                .json(&json!({"database_path": db_path, "game_path": game, "format_id": "renpy"})),
            200,
        )
        .await;
        let reopened = response(client.get(&endpoint).query(&owner), 200).await;
        assert_eq!(reopened["groups"][0]["note"], "Uses formal speech.");
        assert_eq!(
            state.db.get_project_metadata("unrelated").unwrap(),
            Some(json!({"keep": true}))
        );
        // Membership is rebuilt from current entries, including newly saved rows.
        let mut extra = StringEntry::new("explicit", "Hello", "text.json".into());
        extra.metadata.insert("speaker".into(), json!("Clara"));
        state.db.save_entries(&[extra]).unwrap();
        let listed = response(client.get(&endpoint).query(&owner), 200).await;
        assert_eq!(
            listed["groups"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g["count"].as_u64().unwrap())
                .sum::<u64>(),
            4
        );
        write["speaker_id"] = json!("unknown");
        response(client.put(&note_url).json(&write), 404).await;
        assert_eq!(
            state.db.get_project_metadata(NOTES_METADATA_KEY).unwrap(),
            saved
        );
        write["speaker_id"] = alice["id"].clone();
        write["note"] = json!("");
        response(client.put(&note_url).json(&write), 200).await;
        let db = Database::open(&db_path).unwrap();
        assert_eq!(
            db.get_project_metadata(NOTES_METADATA_KEY)
                .unwrap()
                .unwrap(),
            json!({})
        );
        assert_eq!(
            db.get_project_metadata("unrelated").unwrap(),
            Some(json!({"keep": true}))
        );
        *state.current_project.write().await = None;
        response(client.put(&note_url).json(&write), 400).await;
        assert_eq!(
            state
                .db
                .get_project_metadata(NOTES_METADATA_KEY)
                .unwrap()
                .unwrap(),
            json!({})
        );
        handle.abort();
    }
}
