# Hyphenation pattern sources

Pattern data for the layout composer (`docs/layout-v1.md` section 6.4) comes
unmodified from the TeX hyphenation project
(<https://github.com/hyphenation/tex-hyphen>, `hyph-utf8/.../patterns/txt`).
Each file is stored gzip-compressed in `assets/hyphenation/` and decoded on
first use. `sha256` is the checksum of the decompressed source file.

| Pattern set | Language tags | License | sha256 |
| --- | --- | --- | --- |
| `en-us` | en, en-US | Knuth-style notice (copy and distribute unmodified, keep notice) | `0f57318b878b132547ae92db39a6e1d1cf2a05d9008874955d6ecb910007a463` |
| `en-gb` | en-GB | MIT | `651b89b4a0d4051995db8586e81ab109a4c7395d7706c06483893ebab5af013b` |
| `de-1996` | de | MIT | `0570839683e4843a6210b188e2a586fe06f2b6b73952ec8106a634209d735adb` |
| `fr` | fr | MIT | `fd40a89e9e8eb0784a0919e50378814843e44afa41c5beaf0a85f6562820f6c2` |
| `es` | es | MIT/X11 | `7ae8fa8d6a61ea4111c4a0412d019b62e7c07a62673121d0b711c5bc08d8e00c` |
| `it` | it | LPPL 1.3 or MIT | `ce650cffad4e81ecaf04591f3274a8ed909e82c587698c55e33b00e2aa7746de` |
| `pt` | pt | BSD-3-Clause | `37cab85eee09030843f74d7e34453422b69404491abb0d8fa61ad42539b22ee8` |
| `nl` | nl | MIT | `ac5795c1d6cd7027e6ec3a9e58e173789f999669e40ffd81755a4db7d3214165` |
| `sv` | sv | LPPL (distributed unmodified) | `1fbf98f5d316712fd4e86f564643ad7341184f9ce8b30e0e18d06eceb4f52a79` |
| `da` | da | LPPL 1.3 or MIT | `a093ef99375717fe34d261b05d2fdc8e64f8a9f741578c020204374f4c39a20c` |
| `nb` | nb, no | Knuth-style notice (copy and distribute unmodified, keep notice) | `e080c77da3d0f8de7e464f69082f5e2485e43ed349a2a7018b84e0adef785806` |
| `fi` | fi | Freely distributable ("Patterns may be freely distributed") | `ccabb0b3f403eca9999c0cd73b75d16b0220f2052d8f6aad2f98b5f1854dc027` |
| `pl` | pl | MIT | `4b9c3583f176efb76dccee7e630ac7516567abd96dcbc3ebaa2388e795774b48` |
| `ru` | ru | LPPL (distributed unmodified) | `12fa91c83d56ffefa589cf022d9f24ec5a5f4035b4c09484cf4270128c2c162a` |
| `uk` | uk | MIT | `48c8640eb073bee879fb3d57cd91068bf70129b0bb8f7f5db6ecf9e056b57d1e` |
| `el-monoton` | el | LPPL or MIT | `ae58ac05e9c88dcfa6a2ebecb2fd6aebb08c3f36c4bf3c4e96984c0a2a71ad2e` |
| `tr` | tr | LPPL (distributed unmodified) | `0e7dda13d183f667e3eca0f7ea426dbe51458ca6072b878eb35102966ec22f03` |

`hyph-en-us` also ships an exception list (`en-us.hyp.gz`).

## Not bundled

- **Czech (`cs`)**: the upstream pattern file is GPL-licensed, which is not
  compatible with this project's `MIT OR Apache-2.0` license. Czech text
  composes without hyphenation and reports `hyphenation-unavailable`; a user
  dictionary or an embedded document pattern set can supply it.

## Rules

- Only patterns whose license permits redistribution inside an MIT/Apache-2.0
  binary are bundled. LPPL-only sets are distributed unmodified, as the LPPL
  requires; do not edit them in place.
- Bundled data stays at or below 1.5 MiB compressed in total (currently
  about 0.4 MiB).
- Adding or updating a set changes composition: bump `compose::ENGINE`, add
  goldens and record the new checksum here.
