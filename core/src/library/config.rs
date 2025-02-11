use crate::{
	prisma::{file_path, indexer_rule, PrismaClient},
	util::{
		db::{maybe_missing, uuid_to_bytes},
		migrator::{Migrate, MigratorError},
	},
};

use sd_p2p::{spacetunnel::Identity, PeerId};
use sd_prisma::prisma::node;

use std::{path::PathBuf, sync::Arc};

use prisma_client_rust::not;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;
use tracing::error;
use uuid::Uuid;

use super::name::LibraryName;

/// LibraryConfig holds the configuration for a specific library. This is stored as a '{uuid}.sdlibrary' file.
#[derive(Debug, Serialize, Deserialize, Clone)] // If you are adding `specta::Type` on this your probably about to leak the P2P private key
pub struct LibraryConfig {
	/// name is the display name of the library. This is used in the UI and is set by the user.
	pub name: LibraryName,
	/// description is a user set description of the library. This is used in the UI and is set by the user.
	pub description: Option<String>,
	/// P2P identity of this library.
	pub identity: Vec<u8>,
	/// Id of the current node
	pub node_id: Uuid,
	// /// is_encrypted is a flag that is set to true if the library is encrypted.
	// #[serde(default)]
	// pub is_encrypted: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Type)]
pub struct SanitisedLibraryConfig {
	pub name: LibraryName,
	pub description: Option<String>,
	pub node_id: Uuid,
}

impl From<LibraryConfig> for SanitisedLibraryConfig {
	fn from(config: LibraryConfig) -> Self {
		Self {
			name: config.name,
			description: config.description,
			node_id: config.node_id,
		}
	}
}

impl LibraryConfig {
	pub fn new(name: LibraryName, node_id: Uuid) -> Self {
		Self {
			name,
			description: None,
			identity: Identity::new().to_bytes().to_vec(),
			node_id,
		}
	}
}

#[async_trait::async_trait]
impl Migrate for LibraryConfig {
	const CURRENT_VERSION: u32 = 5;

	type Ctx = (Uuid, PeerId, Arc<PrismaClient>);

	fn default(path: PathBuf) -> Result<Self, MigratorError> {
		Err(MigratorError::ConfigFileMissing(path))
	}

	async fn migrate(
		to_version: u32,
		config: &mut serde_json::Map<String, serde_json::Value>,
		(node_id, peer_id, db): &Self::Ctx,
	) -> Result<(), MigratorError> {
		match to_version {
			0 => {}
			1 => {
				let rules = vec![
					format!("No OS protected"),
					format!("No Hidden"),
					format!("No Git"),
					format!("Only Images"),
				];

				db._batch(
					rules
						.into_iter()
						.enumerate()
						.map(|(i, name)| {
							db.indexer_rule().update_many(
								vec![indexer_rule::name::equals(Some(name))],
								vec![indexer_rule::pub_id::set(uuid_to_bytes(Uuid::from_u128(
									i as u128,
								)))],
							)
						})
						.collect::<Vec<_>>(),
				)
				.await?;
			}
			2 => {
				config.insert(
					"identity".into(),
					Value::Array(
						Identity::new()
							.to_bytes()
							.into_iter()
							.map(|v| v.into())
							.collect(),
					),
				);
			}
			// The fact I have to migrate this hurts my soul
			3 => {
				if db.node().count(vec![]).exec().await? != 1 {
					return Err(MigratorError::Custom(
						"Ummm, there are too many nodes in the database, this should not happen!"
							.into(),
					));
				}

				db.node()
					.update_many(
						vec![],
						vec![
							node::pub_id::set(node_id.as_bytes().to_vec()),
							node::node_peer_id::set(Some(peer_id.to_string())),
						],
					)
					.exec()
					.await?;

				config.insert("node_id".into(), Value::String(node_id.to_string()));
			}
			4 => {} // -_-
			5 => loop {
				let paths = db
					.file_path()
					.find_many(vec![not![file_path::size_in_bytes::equals(None)]])
					.take(500)
					.select(file_path::select!({ id size_in_bytes }))
					.exec()
					.await?;

				if paths.is_empty() {
					break;
				}

				db._batch(
					paths
						.into_iter()
						.filter_map(|path| {
							maybe_missing(path.size_in_bytes, "file_path.size_in_bytes")
								.map_or_else(
									|e| {
										error!("{e:#?}");
										None
									},
									Some,
								)
								.map(|size_in_bytes| {
									let size = if let Ok(size) = size_in_bytes.parse::<u64>() {
										Some(size.to_be_bytes().to_vec())
									} else {
										error!(
											"File path <id='{}'> had invalid size: '{}'",
											path.id, size_in_bytes
										);
										None
									};

									db.file_path().update(
										file_path::id::equals(path.id),
										vec![
											file_path::size_in_bytes_bytes::set(size),
											file_path::size_in_bytes::set(None),
										],
									)
								})
						})
						.collect::<Vec<_>>(),
				)
				.await?;
			},
			v => unreachable!("Missing migration for library version {}", v),
		}

		Ok(())
	}
}

// used to return to the frontend with uuid context
#[derive(Serialize, Deserialize, Debug, Type)]
pub struct LibraryConfigWrapped {
	pub uuid: Uuid,
	pub config: SanitisedLibraryConfig,
}
