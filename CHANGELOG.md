
## [0.3.0](https://github.com/QaidVoid/zsync-rs/compare/0.2.2...0.3.0) - 2026-10-26

### Added

- *(cli)* Retry a failed download with growing waits - ([773ef3d](https://github.com/QaidVoid/zsync-rs/commit/773ef3da387a7ce0a75608b46add88ee290d5a69))
- Report progress while scanning seed files - ([bb27967](https://github.com/QaidVoid/zsync-rs/commit/bb27967c233bf0019baf5a8a5faa39fa21f19cc7))

### Fixed

- *(assembly)* Ignore the query when resolving relative URLs - ([622f7d4](https://github.com/QaidVoid/zsync-rs/commit/622f7d4ef6fdd2948f47773d424663be282e681c))
- *(assembly)* Flush the output to disk before renaming it - ([8607324](https://github.com/QaidVoid/zsync-rs/commit/8607324c66fce8202841520674a2c0fe7a12a82d))
- *(assembly)* Keep the replaced file's permissions - ([39672d8](https://github.com/QaidVoid/zsync-rs/commit/39672d8c4febaa73ff8a3c8ad8817be8b743cddc))
- *(cli)* Skip an empty rescan and show the --range-gap default once - ([8cb386f](https://github.com/QaidVoid/zsync-rs/commit/8cb386f438a5b5161c831d6bc9b3d36153fe2a2d))
- *(http)* Reuse connections across range requests - ([71bb439](https://github.com/QaidVoid/zsync-rs/commit/71bb43998befb5d43ce8ee91d5f69bf0daa3cc43))
- *(matcher)* Report only blocks a scan newly finds - ([8d83339](https://github.com/QaidVoid/zsync-rs/commit/8d833398d9e1033f2b7f64a2e22770f338f3ccf1))
- Retry only downloads that failed on a transient error - ([ec627e5](https://github.com/QaidVoid/zsync-rs/commit/ec627e51466cbdc8e725102920615512ce645160))
- Say blocks failed to verify instead of that they are missing - ([b944b3b](https://github.com/QaidVoid/zsync-rs/commit/b944b3bdcd9ec3aead4962639106922c7a8d6374))
- Make progress linear and harden block size and range handling ([#14](https://github.com/QaidVoid/zsync-rs/pull/14)) - ([6bbb4dd](https://github.com/QaidVoid/zsync-rs/commit/6bbb4dd263e697501677b403998e187cdc5aee88))

### Other

- *(deps)* Upgrade dependencies - ([6ca48ec](https://github.com/QaidVoid/zsync-rs/commit/6ca48ec89b805bfd08a0c7f2573e6f901a549765))
- *(http)* Follow a range URL's redirect only once - ([b2a6b94](https://github.com/QaidVoid/zsync-rs/commit/b2a6b940f0bdb4b0b6546b2a162aaec1a2b5cc50))
- Lower the default range gap to 16 KiB - ([f2fd34b](https://github.com/QaidVoid/zsync-rs/commit/f2fd34bb14749b24c0943ef8593c420adeb14912))
- Scan seed files across all cores - ([51d6f0e](https://github.com/QaidVoid/zsync-rs/commit/51d6f0e560f21847f4995c2a0c133f95e5a8de4b))

## [0.2.2](https://github.com/QaidVoid/zsync-rs/compare/0.2.1...0.2.2) - 2026-10-26

### Added

- *(assembly)* Expose the control file block size - ([580500f](https://github.com/QaidVoid/zsync-rs/commit/580500f04b47eb03c3eb64de308d6d11cc13ed98))
- *(cli)* Draw progress as a bar on a terminal - ([3f2f466](https://github.com/QaidVoid/zsync-rs/commit/3f2f46673d36f67c371ff3fe219f56d66b01beae))
- *(cli)* Report download progress and add --json events - ([01c5262](https://github.com/QaidVoid/zsync-rs/commit/01c52621d144eb4937020100f366370c14fdab84))

### Fixed

- *(assembly)* Use portable file I/O to build on non-unix ([#10](https://github.com/QaidVoid/zsync-rs/pull/10)) - ([b768863](https://github.com/QaidVoid/zsync-rs/commit/b768863c3504da2482e22a8d22aad52e2742da13))
- *(cli)* Remove the temp file when a download fails - ([05f3c32](https://github.com/QaidVoid/zsync-rs/commit/05f3c32a48ec45c4d0dcb802b99550bc15ee023d))

### Other

- Document the public library API - ([3533208](https://github.com/QaidVoid/zsync-rs/commit/3533208ed78fb62b35e4a1a418c134fd1dbec2a0))

## [0.2.1](https://github.com/QaidVoid/zsync-rs/compare/0.2.0...0.2.1) - 2026-09-13

### Fixed

- *(http)* Bound connect and resolve, not the transfer - ([d0ae0da](https://github.com/QaidVoid/zsync-rs/commit/d0ae0da9f12969a63adcb533ba63f4a02befb226))

## [0.2.0](https://github.com/QaidVoid/zsync-rs/compare/0.1.4...0.2.0) - 2026-09-08

### Added

- *(assembly)* Add an abort flag for scans and downloads - ([cfe1b24](https://github.com/QaidVoid/zsync-rs/commit/cfe1b24a72f54b285413f6a25a74ddefc6892874))

### Fixed

- *(assembly)* Drop the temp file when the assembly is dropped - ([9894bc0](https://github.com/QaidVoid/zsync-rs/commit/9894bc03d86f550ad3d1a8e2bbbb5e212ffa7ae4))

### Other

- Fix build provenance attestation and bump actions - ([c90c5f3](https://github.com/QaidVoid/zsync-rs/commit/c90c5f371e0ed4e269a3fd286f5f65302828edb0))
- Use crates.io trusted publishing for release - ([932949c](https://github.com/QaidVoid/zsync-rs/commit/932949cf69036137e323ed6a56c377ecdba4758c))

## [0.1.4](https://github.com/QaidVoid/zsync-rs/compare/0.1.3...0.1.4) - 2026-08-20

### Added

- Allow a caller-supplied HTTP agent - ([ff8739b](https://github.com/QaidVoid/zsync-rs/commit/ff8739b081931adde99be730a99717b2ff57ede7))

### Fixed

- Bound control file checksum preallocation - ([e953c08](https://github.com/QaidVoid/zsync-rs/commit/e953c08defaae7d9e21e545f50f0d1d53aed26b5))
- Bound the control file header section - ([23e5ed6](https://github.com/QaidVoid/zsync-rs/commit/23e5ed65aee67d663294059a6c71df8c2c390f63))
- Cap a range response at the requested length - ([252fc21](https://github.com/QaidVoid/zsync-rs/commit/252fc21ed8db18e6103df19058d52f817fc54ffb))

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
