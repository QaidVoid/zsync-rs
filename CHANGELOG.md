
## [0.1.3](https://github.com/QaidVoid/zsync-rs/compare/0.1.2...0.1.3) - 2026-04-14

### Added

- Add zsyncmake support - ([85001b0](https://github.com/QaidVoid/zsync-rs/commit/85001b022ef42bbd896e630e1566552bd6c8e94a))

### Other

- Add zsyncmake binary to release - ([f509660](https://github.com/QaidVoid/zsync-rs/commit/f5096605ef537bbad940cb0c32b29c75464a7a5a))

## [0.1.2](https://github.com/QaidVoid/zsync-rs/compare/0.1.1...0.1.2) - 2026-03-25

### Other

- Use streaming to reduce memory usage - ([63585a4](https://github.com/QaidVoid/zsync-rs/commit/63585a44118778f1fea7646312e059189e3c0fb2))

## [0.1.1](https://github.com/QaidVoid/zsync-rs/compare/0.1.0...0.1.1) - 2026-03-24

### Added

- Add progress callback support for downloads - ([45ea884](https://github.com/QaidVoid/zsync-rs/commit/45ea884c758382f715353d4323c7dd473fe8d251))

### Other

- Add release profile - ([5de30ce](https://github.com/QaidVoid/zsync-rs/commit/5de30ce78bab9700a7cb10dfb9d3a3f0955714c4))

## [0.1.0] - 2026-03-23

### Added

- Configurable range merge gap threshold - ([14dcb51](https://github.com/QaidVoid/zsync-rs/commit/14dcb5165dc282c1512d17c234f8eb6e350f648e))
- Add multiple seed files and self-referential scanning - ([f896959](https://github.com/QaidVoid/zsync-rs/commit/f89695962c88917ed8c96001fbdda378480b4e63))
- Add human-friendly size formatting and progress percentages to CLI - ([faa433e](https://github.com/QaidVoid/zsync-rs/commit/faa433e36d1db08887498c9a988bc3ee64ade6c3))
- Add CLI tool for zsync downloads - ([4f31d62](https://github.com/QaidVoid/zsync-rs/commit/4f31d627b21f5321828a707617482213219e6190))
- Add file assembly module - ([cdd0d6d](https://github.com/QaidVoid/zsync-rs/commit/cdd0d6d505b7e841229801637584fd7511b63234))
- Add HTTP client with range request support - ([b2be7b7](https://github.com/QaidVoid/zsync-rs/commit/b2be7b7f526aea2aab405315ac72241a85072a07))
- Add block matching algorithm - ([974fa01](https://github.com/QaidVoid/zsync-rs/commit/974fa01a8cf0c38f4e9ab40bbda40a38ceac58fb))
- Add initial library with control file parser, rsum, and checksums - ([5e3ed35](https://github.com/QaidVoid/zsync-rs/commit/5e3ed35e97967701cb66ff8cec7748989e7aeb5e))

### Fixed

- Add num_blocks limit in parser and update fuzz targets - ([cb1749b](https://github.com/QaidVoid/zsync-rs/commit/cb1749be72459d07973862f884a5ae5f41452fb0))
- Implement sequential block chaining for accurate matching - ([6e733ed](https://github.com/QaidVoid/zsync-rs/commit/6e733ed71cf10abe8818c1dfeac1960c5f7bab47))
- Properly handle seq_matches for consecutive block matching - ([ec89459](https://github.com/QaidVoid/zsync-rs/commit/ec8945913753243d73d56af3d8a87e51abeac4f4))
- Handle variable rsum_bytes in control file parsing - ([df4d761](https://github.com/QaidVoid/zsync-rs/commit/df4d761e391cce51f9fb3f62707672fb04d3ed8e))
- Handle partial last block with zero padding - ([e26fd62](https://github.com/QaidVoid/zsync-rs/commit/e26fd626c266065d6451c9afa94e31dca42aeacb))
- Resolve relative URLs against zsync file base URL - ([1c3bc13](https://github.com/QaidVoid/zsync-rs/commit/1c3bc13b6d2005a22cdb5165a907e6adeab92751))

### Other

- Add release workflow - ([c797ddb](https://github.com/QaidVoid/zsync-rs/commit/c797ddbe78989b1d4d1963f8d7faf4af09e76bae))
- Add package metadata to Cargo.toml - ([3bb6b81](https://github.com/QaidVoid/zsync-rs/commit/3bb6b81b29dc1701d846a32f4eb1e908ff66a38b))
- Add license - ([f7b2396](https://github.com/QaidVoid/zsync-rs/commit/f7b239684414bf21a77037bf527f4e6202c80186))
- Add README - ([0f153ab](https://github.com/QaidVoid/zsync-rs/commit/0f153abccda6275a5cf876a86c1724238449be82))
- Add parallel block matching for large files - ([f639bfc](https://github.com/QaidVoid/zsync-rs/commit/f639bfc38b7368a47ae19c4c93ec99056a76bf70))
- Inline checksums, use pwrite, reuse download buffer - ([36bbb75](https://github.com/QaidVoid/zsync-rs/commit/36bbb75d76cbc496f3b969c1b843bb3424a049ff))
- Merge byte ranges to minimize HTTP requests - ([ca52c1d](https://github.com/QaidVoid/zsync-rs/commit/ca52c1d475ea933d6aeb21fd36c6747bd5308343))
- Replace HashMap with flat hash table and bithash - ([808b4da](https://github.com/QaidVoid/zsync-rs/commit/808b4da5249b260524e8d6921acb110a86010402))
- Add AGENTS.md with project guidelines - ([9b83078](https://github.com/QaidVoid/zsync-rs/commit/9b83078ff55c353e634b06124903a5b54332129f))
