//! Projecting committed change events onto the editor's view state.
//!
//! An edit that arrives over the session socket is already in the project by
//! the time we see it — the command engine committed it. What is left is the
//! state the project does not own: what is selected, and which waveforms are
//! cached. Both are keyed by entity ID, so a change that only *updates* an
//! entity leaves them alone; only a removal has to be followed.

use jutsu_audio_commands::{ChangeEvent, ChangeKind, EntityKind};

/// True when `changes` removed the entity with this ID.
pub fn removes(changes: &[ChangeEvent], entity_kind: EntityKind, entity_id: &str) -> bool {
    changes.iter().any(|change| {
        change.kind == ChangeKind::Removed
            && change.entity_kind == entity_kind
            && change.entity_id == entity_id
    })
}

/// The IDs of every asset these changes removed, as they appear on the wire.
pub fn removed_assets(changes: &[ChangeEvent]) -> Vec<&str> {
    changes
        .iter()
        .filter(|change| {
            change.kind == ChangeKind::Removed && change.entity_kind == EntityKind::Asset
        })
        .map(|change| change.entity_id.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(kind: ChangeKind, entity_kind: EntityKind, entity_id: &str) -> ChangeEvent {
        ChangeEvent {
            sequence: 0,
            kind,
            entity_kind,
            entity_id: entity_id.into(),
        }
    }

    #[test]
    fn a_removed_clip_is_reported_so_the_selection_can_let_go_of_it() {
        let changes = [change(ChangeKind::Removed, EntityKind::Clip, "clip-1")];
        assert!(removes(&changes, EntityKind::Clip, "clip-1"));
    }

    #[test]
    fn an_updated_clip_keeps_its_selection() {
        let changes = [change(ChangeKind::Updated, EntityKind::Clip, "clip-1")];
        assert!(
            !removes(&changes, EntityKind::Clip, "clip-1"),
            "an update must not drop the selection — that is what stable IDs are for"
        );
    }

    #[test]
    fn removal_is_matched_by_kind_as_well_as_id() {
        let changes = [change(ChangeKind::Removed, EntityKind::Asset, "shared-id")];
        assert!(removes(&changes, EntityKind::Asset, "shared-id"));
        assert!(!removes(&changes, EntityKind::Clip, "shared-id"));
    }

    #[test]
    fn every_removed_asset_is_listed_so_its_waveform_can_be_dropped() {
        let changes = [
            change(ChangeKind::Removed, EntityKind::Asset, "asset-1"),
            change(ChangeKind::Added, EntityKind::Asset, "asset-2"),
            change(ChangeKind::Removed, EntityKind::Clip, "clip-1"),
            change(ChangeKind::Removed, EntityKind::Asset, "asset-3"),
        ];
        assert_eq!(removed_assets(&changes), ["asset-1", "asset-3"]);
    }
}
