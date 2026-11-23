# 第三方依赖与来源

本项目首轮使用官方包，无 Sunshine / Moonlight / EasyTier 源码或二进制。

| 直接依赖 | 来源 | 声明许可 |
|---|---|---|
| Tauri / Tauri Build / API / CLI | https://github.com/tauri-apps/tauri | MIT OR Apache-2.0 |
| windows-rs | https://github.com/microsoft/windows-rs | MIT OR Apache-2.0 |
| serde / serde_json | https://github.com/serde-rs | MIT OR Apache-2.0 |
| thiserror | https://github.com/dtolnay/thiserror | MIT OR Apache-2.0 |
| webrtc / rtc / rtc-* 0.21.1 | https://github.com/webrtc-rs/webrtc / https://github.com/webrtc-rs/rtc | MIT OR Apache-2.0 |
| Tokio / async-trait | https://github.com/tokio-rs/tokio / https://github.com/dtolnay/async-trait | MIT / MIT OR Apache-2.0 |
| bytes / uuid | https://github.com/tokio-rs/bytes / https://github.com/uuid-rs/uuid | MIT / Apache-2.0 OR MIT |
| hmac / sha2 / sha1（sha1仅server-only TURN REST兼容） | https://github.com/RustCrypto/MACs / https://github.com/RustCrypto/hashes | MIT OR Apache-2.0 |
| base64 / zeroize（服务端票据与秘密内存清理） | https://github.com/marshallpierce/rust-base64 / https://github.com/RustCrypto/utils | MIT OR Apache-2.0 |
| ring 0.17.14（传递密码依赖） | https://github.com/briansmith/ring | Apache-2.0 AND ISC |
| ed25519-dalek 2.2.0 / curve25519-dalek 4.1.3（P2-01本机身份） | https://github.com/dalek-cryptography/curve25519-dalek | BSD-3-Clause |
| curve25519-dalek-derive 0.1.1 | https://github.com/dalek-cryptography/curve25519-dalek | MIT OR Apache-2.0 |
| ed25519 2.2.3 / signature 2.2.0 | https://github.com/RustCrypto/signatures / https://github.com/RustCrypto/traits | MIT OR Apache-2.0 |
| base64ct / const-oid / der / pkcs8 / spki（新锁条目，部分为未启用的可选依赖） | https://github.com/RustCrypto/formats | MIT OR Apache-2.0 |
| fiat-crypto 0.2.9（锁中可选后端） | https://github.com/mit-plv/fiat-crypto | MIT OR Apache-2.0 OR BSD-1-Clause |
| Axum 0.8.8 / axum-server 0.8.0（隔离实验信令） | https://github.com/tokio-rs/axum / https://github.com/programatik29/axum-server | MIT |
| tokio-tungstenite / tungstenite 0.28.0 | https://github.com/snapview/tokio-tungstenite / https://github.com/snapview/tungstenite-rs | MIT OR Apache-2.0 |
| rustls 0.23.46 / rustls-pemfile 2.2.0 | https://github.com/rustls/rustls / https://github.com/rustls/pemfile | Apache-2.0 OR ISC OR MIT / Apache-2.0 OR ISC OR MIT |
| rcgen 0.14.10（本机TLS开发测试） | https://github.com/rustls/rcgen | MIT OR Apache-2.0 |
| Vue / Vue Test Utils / vue-tsc | https://github.com/vuejs | MIT |
| Vite / Vitest | https://github.com/vitejs / https://github.com/vitest-dev | MIT |
| Tailwind CSS | https://github.com/tailwindlabs/tailwindcss | MIT |
| TypeScript | https://github.com/microsoft/TypeScript | Apache-2.0 |
| Playwright | https://github.com/microsoft/playwright | Apache-2.0 |
| jsdom / Prettier | https://github.com/jsdom/jsdom / https://github.com/prettier/prettier | MIT |
| RFC5769 STUN测试向量（仅开发测试） | https://www.rfc-editor.org/rfc/rfc5769.html | IETF Trust Simplified BSD，附scripts/RFC5769_TEST_VECTOR_LICENSE.txt |

`vendor/webrtc-0.21.1` 来自锁定的 crates.io 发布包，保留 MIT / Apache-2.0 许可证；RemoteHub 只增加单连接的 Host 候选收集开关，默认保持上游行为。

精确版本由 Cargo.lock 和 pnpm-lock.yaml 固定。锁文件含全部传递依赖，分发前仍需按最终二进制和全部传递依赖生成完整 SBOM/许可附件；本文不是完整分发法律审查。P0-03 已通过系统 Media Foundation 编码 H.264，不随产品附带外部编码器；FFmpeg 只作为本机开发解码/封装验证工具，不作为客户端运行依赖打包。

开发工具不打包分发：Rust 官方工具链，Microsoft MSVC / Windows SDK；其自身适用对应许可。项目内临时准备工具脚本参考 https://gist.github.com/mmozeiko/7f3162ec2988e81e56d5c4e22cde9977 ，从 Microsoft 清单下载并逐文件 SHA-256 校验，仅用于本机 `.tools` 目录，排除于交付。此脚本不是产品运行依赖。
