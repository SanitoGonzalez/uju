# uju

Experimental ideas for seamless MMO server

# 1. RUDP

RUDP is not a brand new idea, but would be essential for realtime actions which require low latency.

# 2. Server Meshing

- cluster > node > shard > cell
- *shard*: shard-per-core with shared-nothing model.
  (ref. [compio](https://compio.rs/), [seastar](https://seastar.io/shared-nothing/))

# X. Idea Notes

- [spatial network](https://en.wikipedia.org/wiki/Spatial_network):
    - shard worlds of ECS, letting each thread run the shard
    - use position as a shard criteria
    - migrate entities on shard criteria
    - migrate entities if cross-shard interaction exceeds the threshold
