# Vendored Uniswap V2 sources

Copied by `scripts/vendor-uniswap-v2.sh` (m4-evm task 7.3, design D14); rerun it with
`--check` to reproduce these files byte for byte. They keep their upstream licence,
GPL-3.0-or-later (see each LICENSE), and are compiled with the upstream settings: solc
0.5.16 (v2-core) and 0.6.6 (v2-periphery), optimizer 999999 runs, EVM version istanbul.

| Directory | Source | Version |
|---|---|---|
| `v2-core/` | https://github.com/Uniswap/v2-core | commit `4dd59067c76dea4a0e8e4bfdda41877a6b16dedc` (tag v1.0.1) |
| `v2-periphery/` | https://github.com/Uniswap/v2-periphery | commit `0335e8f7e1bd1e8d8329fd300aea2ef2f36dd19f` |
| `uniswap-lib/` | https://registry.npmjs.org/@uniswap/lib/-/lib-4.0.1-alpha.tgz | @uniswap/lib 4.0.1-alpha, `sha512-f6UIliwBbRsgVLxIaBANF6w09tYqc6Y/qXdsrbEmXHyFA7ILiKrIwRFXe1yOg8M3cksgVsO9N7yuL2DdCGQKBA==` |

## Change to upstream

`v2-periphery/contracts/libraries/UniswapV2Library.sol`: the pair init-code hash in
`pairFor` is `30df050da9efe464e849f14dc53332691199266cd01cc61bd0cc9f8548fa5539` (upstream `96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f`), the
keccak-256 of `UniswapV2Pair`'s init code as this project compiles it; the metadata hash
embedded in the bytecode depends on source paths. Upstream file SHA-256: `4f83e9334f833568fa47b36e9ceca435f6c2962760a0596b043c4e538d0fd9f2`.

## Files

| File | SHA-256 |
|---|---|
| `uniswap-lib/LICENSE` | 3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986 |
| `uniswap-lib/contracts/libraries/TransferHelper.sol` | 22b87fd425d590e533ab7e52478cf72bdc4bde2672e0977c7eff7742e8f0737d |
| `v2-core/LICENSE` | 3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986 |
| `v2-core/contracts/UniswapV2ERC20.sol` | b8ce88929ce2c93b8fce385f8d57701b1a64c600a95317ea5f3561d36297be32 |
| `v2-core/contracts/UniswapV2Factory.sol` | e0cef3e874a68cbcc5986451b1fec180ba5ff5699f27a256b7c10fafefe36b99 |
| `v2-core/contracts/UniswapV2Pair.sol` | 43a5421b31415868367b62bfa161ca10bcee03778873faad905f5a3e2cce9cbd |
| `v2-core/contracts/interfaces/IERC20.sol` | 2b63f199f838028184efefbcfd6cf2b9192624c3dae5dc1116ecbb15c36a67e8 |
| `v2-core/contracts/interfaces/IUniswapV2Callee.sol` | ecfa434d468bb1888124d8c34b0b8483f4ea1e5cd63f0874c8b28aa4305c1dbc |
| `v2-core/contracts/interfaces/IUniswapV2ERC20.sol` | 7948069aea398dec4b7e771ffc749a24fdce3a64673a1b14deae960e7a54c603 |
| `v2-core/contracts/interfaces/IUniswapV2Factory.sol` | 51d056199e3f5e41cb1a9f11ce581aa3e190cc982db5771ffeef8d8d1f962a0d |
| `v2-core/contracts/interfaces/IUniswapV2Pair.sol` | 29c75e69ce173ff8b498584700fef76bc81498c1d98120e2877a1439f0c31b5a |
| `v2-core/contracts/libraries/Math.sol` | e4a9d451964a0689be2b244322a353de143ca4248d8736d91aca4ffadca4325f |
| `v2-core/contracts/libraries/SafeMath.sol` | 4b1c95ff75de7342e0fadff58064820a4eb7c2fcb422a75b4994980ce8e216ae |
| `v2-core/contracts/libraries/UQ112x112.sol` | 6633b57b0723b1d72e08cc3e8b29f0af838294e59863b6cdcce95a141ed02cdb |
| `v2-core/contracts/test/ERC20.sol` | e11b370820584a64b1ef5b498a348349f311787ba77b3b0a632abc1110dcc7e2 |
| `v2-periphery/LICENSE` | 3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986 |
| `v2-periphery/contracts/UniswapV2Router02.sol` | acabe6d6c9c275aa5c27d8ed59b3d23d21616523071043e89eea7bca474babb8 |
| `v2-periphery/contracts/interfaces/IERC20.sol` | 2b63f199f838028184efefbcfd6cf2b9192624c3dae5dc1116ecbb15c36a67e8 |
| `v2-periphery/contracts/interfaces/IUniswapV2Router01.sol` | 0439ffe0fd4a5e1f4e22d71ddbda76d63d61679947d158cba4ee0a1da60cf663 |
| `v2-periphery/contracts/interfaces/IUniswapV2Router02.sol` | a2900701961cb0b6152fc073856b972564f7c798797a4a044e83d2ab8f0e8d38 |
| `v2-periphery/contracts/interfaces/IWETH.sol` | 60760053849b916b72386fef1126ab69c7482c2aa6b5fb836368eff42905cb30 |
| `v2-periphery/contracts/libraries/SafeMath.sol` | 39e5b4c0bc19b72fa59c2d2177ba5ed6cfadcd76f3f804563011e579367530fb |
| `v2-periphery/contracts/libraries/UniswapV2Library.sol` | a8def9efa7b3476408bc76382e5e31e8d845448615b2191f032b671e0a572cc5 |
| `v2-periphery/contracts/test/WETH9.sol` | 33f13565b207188485030aa20fd1f38d52d8c455cc64720560fb38149dcb4137 |
