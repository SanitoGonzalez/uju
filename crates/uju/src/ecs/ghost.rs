use std::collections::{HashMap, HashSet};

use crate::ecs::entity::{Entity, UniversalEntity};
use crate::mesh::{node, shard};

pub struct Registry {
    inbound: HashMap<UniversalEntity, Entity>,
    inbound_bulk: HashMap<(node::Id, shard::Id), HashSet<Entity>>,
    outbound: HashMap<Entity, UniversalEntity>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            inbound: HashMap::new(),
            inbound_bulk: HashMap::new(),
            outbound: HashMap::new(),
        }
    }

    pub fn insert_inbound(&mut self, universal: UniversalEntity, local: Entity) -> Option<Entity> {
        let old = self.inbound.insert(universal, local);
        self.inbound_bulk
            .entry(universal.tuple())
            .or_default()
            .insert(local);
        old
    }

    pub fn remove_inbound(&mut self, universal: &UniversalEntity) -> Option<Entity> {
        let old = self.inbound.remove(universal)?;
        if let Some(bulk) = self.inbound_bulk.get_mut(&universal.tuple()) {
            bulk.remove(&old);
        }
        Some(old)
    }

    pub fn insert_outbound(
        &mut self,
        local: Entity,
        universal: UniversalEntity,
    ) -> Option<UniversalEntity> {
        self.outbound.insert(local, universal)
    }

    pub fn remove_outbound(&mut self, local: &Entity) -> Option<UniversalEntity> {
        self.outbound.remove(local)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
