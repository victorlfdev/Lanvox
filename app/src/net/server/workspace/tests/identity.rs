use super::*;

pub(super) fn rotation(
    user: UserId,
    old: &ring::signature::Ed25519KeyPair,
    new: &ring::signature::Ed25519KeyPair,
    nonce: [u8; 32],
) -> WorkspaceCommand {
    use ring::signature::KeyPair;
    let old_key: [u8; 32] = old.public_key().as_ref().try_into().unwrap();
    let new_key: [u8; 32] = new.public_key().as_ref().try_into().unwrap();
    let transcript = identity_rotation_transcript(user, &old_key, &new_key, &nonce);
    WorkspaceCommand::RotateIdentity {
        old_public_key: old_key,
        new_public_key: new_key,
        nonce,
        old_signature: old.sign(&transcript).as_ref().to_vec(),
        new_signature: new.sign(&transcript).as_ref().to_vec(),
    }
}

#[test]
fn rotation_requires_both_proofs_is_idempotent_and_revokes_the_old_key() {
    use ring::signature::KeyPair;
    let (_, old_doc) = crate::auth::Identity::generate().unwrap();
    let (_, new_doc) = crate::auth::Identity::generate().unwrap();
    let old = ring::signature::Ed25519KeyPair::from_pkcs8(&old_doc).unwrap();
    let new = ring::signature::Ed25519KeyPair::from_pkcs8(&new_doc).unwrap();
    let old_key: [u8; 32] = old.public_key().as_ref().try_into().unwrap();
    let new_key: [u8; 32] = new.public_key().as_ref().try_into().unwrap();
    let mut st = fixture();
    st.public_keys.insert(1, old_key);
    let mut bad = rotation(1, &old, &new, [7; 32]);
    if let WorkspaceCommand::RotateIdentity { new_signature, .. } = &mut bad {
        new_signature[0] ^= 1;
    }
    assert!(st.workspace_command(1, bad).is_err());
    assert_eq!(st.public_keys[&1], old_key);
    let command = rotation(1, &old, &new, [7; 32]);
    assert!(st.workspace_command(2, command.clone()).is_err());
    st.workspace_command(1, command.clone()).unwrap();
    assert_eq!(st.public_keys[&1], new_key);
    assert!(st.is_revoked(&old_key));
    assert!(st.identify("new name", old_key).is_err());
    st.workspace_command(1, command).unwrap();
    assert_eq!(st.workspace.rotations.len(), 1);
    assert_eq!(st.workspace.revoked_keys.len(), 1);
    let mut changed = rotation(1, &old, &new, [8; 32]);
    if let WorkspaceCommand::RotateIdentity { old_signature, .. } = &mut changed {
        old_signature[0] ^= 1;
    }
    assert!(st.workspace_command(1, changed).is_err());
}

#[test]
fn restart_with_stale_owner_key_follows_committed_rotation() {
    use ring::signature::KeyPair;
    let (_, old_doc) = crate::auth::Identity::generate().unwrap();
    let (_, new_doc) = crate::auth::Identity::generate().unwrap();
    let old = ring::signature::Ed25519KeyPair::from_pkcs8(&old_doc).unwrap();
    let new = ring::signature::Ed25519KeyPair::from_pkcs8(&new_doc).unwrap();
    let old_key: [u8; 32] = old.public_key().as_ref().try_into().unwrap();
    let new_key: [u8; 32] = new.public_key().as_ref().try_into().unwrap();
    let mut st = fixture();
    st.public_keys.insert(1, old_key);
    st.options.owner_key = Some(old_key);
    st.workspace_command(1, rotation(1, &old, &new, [5; 32]))
        .unwrap();
    // Simulate a restart: the configuration still names the old key.
    st.options.owner_key = Some(old_key);
    st.follow_owner_rotation();
    assert_eq!(st.options.owner_key, Some(new_key));
    st.follow_owner_rotation();
    assert_eq!(st.options.owner_key, Some(new_key));
}

#[test]
fn failed_rotation_persistence_restores_identity_and_revocation_state() {
    use ring::signature::KeyPair;
    let (_, old_doc) = crate::auth::Identity::generate().unwrap();
    let (_, new_doc) = crate::auth::Identity::generate().unwrap();
    let old = ring::signature::Ed25519KeyPair::from_pkcs8(&old_doc).unwrap();
    let new = ring::signature::Ed25519KeyPair::from_pkcs8(&new_doc).unwrap();
    let old_key: [u8; 32] = old.public_key().as_ref().try_into().unwrap();
    let mut st = fixture();
    st.public_keys.insert(1, old_key);
    st.options.owner_key = Some(old_key);
    st.save_path = Some(
        std::env::temp_dir()
            .join(format!("lanvox-missing-rotation-{}", rand::random::<u64>()))
            .join("state.json"),
    );
    assert!(st
        .workspace_command(1, rotation(1, &old, &new, [9; 32]))
        .is_err());
    assert_eq!(st.public_keys[&1], old_key);
    assert_eq!(st.options.owner_key, Some(old_key));
    assert!(st.workspace.revoked_keys.is_empty());
    assert!(st.workspace.rotations.is_empty());
}
