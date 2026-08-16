use std::collections::{HashMap, HashSet};

use crate::ecs::entity::{Entity, UniversalEntity};
use crate::mesh::{node, shard};

/// What a `link` displaced. `local` is a proxy nothing maps anymore - the caller must despawn it.
#[must_use]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Displaced {
    pub local: Option<Entity>,
    pub universal: Option<UniversalEntity>,
}

pub struct Registry {
    inbound: HashMap<UniversalEntity, Entity>,
    outbound: HashMap<Entity, UniversalEntity>,
    origins: HashMap<(node::Id, shard::Id), HashSet<Entity>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            inbound: HashMap::new(),
            outbound: HashMap::new(),
            origins: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.inbound.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inbound.is_empty()
    }

    /// The local proxy standing for a remote entity, to apply an inbound update
    pub fn local(&self, universal: UniversalEntity) -> Option<Entity> {
        self.inbound.get(&universal).copied()
    }

    /// The remote origin a local proxy stands for, to route a write to its owner
    pub fn universal(&self, local: Entity) -> Option<UniversalEntity> {
        self.outbound.get(&local).copied()
    }

    /// Bind a remote entity to a local proxy, displacing whatever either side was
    /// bound to before. Re-linking an existing pair is a no-op; re-pointing a proxy
    /// to a new origin (entity migration) keeps the proxy and every local reference
    /// to it intact.
    pub fn link(&mut self, universal: UniversalEntity, local: Entity) -> Displaced {
        let old_local = self.inbound.insert(universal, local);
        if let Some(old) = old_local
            && old != local
        {
            self.outbound.remove(&old);
        }

        let old_universal = self.outbound.insert(local, universal);
        if let Some(old) = old_universal
            && old != universal
        {
            self.inbound.remove(&old);
            self.forget_origin(old);
        }

        self.origins
            .entry(universal.tuple())
            .or_default()
            .insert(universal.entity());

        Displaced {
            local: old_local.filter(|&old| old != local),
            universal: old_universal.filter(|&old| old != universal),
        }
    }

    pub fn unlink(&mut self, universal: UniversalEntity) -> Option<Entity> {
        let local = self.inbound.remove(&universal)?;
        self.outbound.remove(&local);
        self.forget_origin(universal);
        Some(local)
    }

    /// Unlink by proxy, for despawn paths that do not know the origin
    pub fn unlink_local(&mut self, local: Entity) -> Option<UniversalEntity> {
        let universal = self.outbound.remove(&local)?;
        self.inbound.remove(&universal);
        self.forget_origin(universal);
        Some(universal)
    }

    /// Drop every link to an origin, returning the orphaned proxies to despawn
    pub fn unlink_origin(&mut self, node: node::Id, shard: shard::Id) -> Vec<Entity> {
        let Some(remotes) = self.origins.remove(&(node, shard)) else {
            return Vec::new();
        };

        remotes
            .into_iter()
            .filter_map(|remote| {
                let local = self
                    .inbound
                    .remove(&UniversalEntity::new(node, shard, remote))?;
                self.outbound.remove(&local);
                Some(local)
            })
            .collect()
    }

    fn forget_origin(&mut self, universal: UniversalEntity) {
        let Some(bucket) = self.origins.get_mut(&universal.tuple()) else {
            return;
        };

        bucket.remove(&universal.entity());
        if bucket.is_empty() {
            self.origins.remove(&universal.tuple());
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::entity::{EntityGeneration, EntityIndex};

    fn entity(index: u32, generation: u32) -> Entity {
        Entity::new(
            EntityIndex::from_bits(index),
            EntityGeneration::from_bits(generation),
        )
    }

    fn universal(node: node::Id, shard: shard::Id, index: u32) -> UniversalEntity {
        UniversalEntity::new(node, shard, entity(index, 1))
    }

    impl Registry {
        // inbound <-> outbound mirror each other and origins indexes exactly the
        // inbound keys - the invariant every mutation must preserve
        fn assert_consistent(&self) {
            assert_eq!(self.inbound.len(), self.outbound.len());
            for (&universal, &local) in &self.inbound {
                assert_eq!(self.outbound.get(&local), Some(&universal));
                assert!(self.origins[&universal.tuple()].contains(&universal.entity()));
            }
            let origins_total: usize = self.origins.values().map(HashSet::len).sum();
            assert_eq!(origins_total, self.inbound.len());
        }
    }

    #[test]
    fn registry_link() {
        let mut registry = Registry::new();
        let remote = universal(3, 7, 5);
        let proxy = entity(91, 1);

        assert!(registry.is_empty());
        assert_eq!(registry.link(remote, proxy), Displaced::default());
        registry.assert_consistent();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.local(remote), Some(proxy)); // apply an update from the origin
        assert_eq!(registry.universal(proxy), Some(remote)); // route a write back to the origin

        assert_eq!(registry.unlink(remote), Some(proxy));
        assert_eq!(registry.unlink(remote), None);
        registry.assert_consistent();

        assert!(registry.is_empty());
        assert_eq!(registry.local(remote), None);
        assert_eq!(registry.universal(proxy), None);
        assert!(registry.origins.is_empty()); // the drained origin leaves no bucket
    }

    #[test]
    fn registry_link_idempotent() {
        let mut registry = Registry::new();
        let remote = universal(3, 7, 5);
        let proxy = entity(91, 1);

        assert_eq!(registry.link(remote, proxy), Displaced::default());
        assert_eq!(registry.link(remote, proxy), Displaced::default());
        registry.assert_consistent();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.local(remote), Some(proxy));
        assert_eq!(registry.universal(proxy), Some(remote));
    }

    #[test]
    fn registry_link_displace_local() {
        let mut registry = Registry::new();
        let remote = universal(3, 7, 5);

        assert_eq!(registry.link(remote, entity(91, 1)), Displaced::default());
        assert_eq!(
            registry.link(remote, entity(92, 1)),
            Displaced {
                local: Some(entity(91, 1)), // unreachable now, caller despawns it
                universal: None,
            }
        );
        registry.assert_consistent();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.local(remote), Some(entity(92, 1)));
        assert_eq!(registry.universal(entity(92, 1)), Some(remote));
        assert_eq!(registry.universal(entity(91, 1)), None);
    }

    #[test]
    fn registry_link_displace_universal() {
        let mut registry = Registry::new();
        let proxy = entity(91, 1);

        // the remote entity migrated from shard 7 to shard 8 - the proxy is re-pointed
        // in place, so local references to it survive the handoff
        let before = universal(3, 7, 5);
        let after = universal(3, 8, 5);

        assert_eq!(registry.link(before, proxy), Displaced::default());
        assert_eq!(
            registry.link(after, proxy),
            Displaced {
                local: None,
                universal: Some(before),
            }
        );
        registry.assert_consistent();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.local(after), Some(proxy));
        assert_eq!(registry.local(before), None);
        assert_eq!(registry.universal(proxy), Some(after));
        assert!(!registry.origins.contains_key(&(3, 7)));
    }

    #[test]
    fn registry_link_displace_crossed() {
        let mut registry = Registry::new();
        let a = universal(3, 7, 5);
        let b = universal(3, 7, 6);

        assert_eq!(registry.link(a, entity(91, 1)), Displaced::default());
        assert_eq!(registry.link(b, entity(92, 1)), Displaced::default());

        // both sides were bound elsewhere - both bindings dissolve
        assert_eq!(
            registry.link(a, entity(92, 1)),
            Displaced {
                local: Some(entity(91, 1)),
                universal: Some(b),
            }
        );
        registry.assert_consistent();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.local(a), Some(entity(92, 1)));
        assert_eq!(registry.local(b), None);
        assert_eq!(registry.universal(entity(92, 1)), Some(a));
        assert_eq!(registry.universal(entity(91, 1)), None);
    }

    #[test]
    fn registry_link_distinct_origins() {
        let mut registry = Registry::new();

        // the same origin-side index on three different origins - the shards allocate
        // indexes independently, so collisions here are the normal case, not an error
        let a = universal(3, 7, 5);
        let b = universal(3, 8, 5);
        let c = universal(4, 7, 5);

        assert_eq!(registry.link(a, entity(91, 1)), Displaced::default());
        assert_eq!(registry.link(b, entity(92, 1)), Displaced::default());
        assert_eq!(registry.link(c, entity(93, 1)), Displaced::default());
        registry.assert_consistent();

        assert_eq!(registry.len(), 3);
        assert_eq!(registry.local(a), Some(entity(91, 1)));
        assert_eq!(registry.local(b), Some(entity(92, 1)));
        assert_eq!(registry.local(c), Some(entity(93, 1)));
    }

    #[test]
    fn registry_link_generation() {
        let mut registry = Registry::new();

        // a respawn at the origin reuses the index with a new generation, and must map
        // to its own proxy rather than aliasing the dead one
        let first = UniversalEntity::new(3, 7, entity(5, 1));
        let second = UniversalEntity::new(3, 7, entity(5, 3));

        assert_eq!(registry.link(first, entity(91, 1)), Displaced::default());
        assert_eq!(registry.link(second, entity(92, 1)), Displaced::default());
        registry.assert_consistent();

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.local(first), Some(entity(91, 1)));
        assert_eq!(registry.local(second), Some(entity(92, 1)));
    }

    #[test]
    fn registry_unlink_local() {
        let mut registry = Registry::new();
        let remote = universal(3, 7, 5);
        let proxy = entity(91, 1);

        assert_eq!(registry.unlink_local(proxy), None);

        _ = registry.link(remote, proxy);
        assert_eq!(registry.unlink_local(proxy), Some(remote));
        assert_eq!(registry.unlink_local(proxy), None);
        registry.assert_consistent();

        assert!(registry.is_empty());
        assert_eq!(registry.local(remote), None);
        assert!(registry.origins.is_empty());
    }

    #[test]
    fn registry_unlink_origin() {
        let mut registry = Registry::new();
        _ = registry.link(universal(3, 7, 5), entity(91, 1));
        _ = registry.link(universal(3, 7, 6), entity(92, 1));
        _ = registry.link(universal(4, 1, 5), entity(93, 1));

        // draining one origin must leave its neighbors untouched
        let orphaned: HashSet<_> = registry.unlink_origin(3, 7).into_iter().collect();
        assert_eq!(orphaned, HashSet::from([entity(91, 1), entity(92, 1)]));
        registry.assert_consistent();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.local(universal(3, 7, 5)), None);
        assert_eq!(registry.universal(entity(91, 1)), None);
        assert_eq!(registry.local(universal(4, 1, 5)), Some(entity(93, 1)));

        assert!(registry.unlink_origin(3, 7).is_empty());
    }
}
