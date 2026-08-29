# Dependencies — scout

crate ごとの用途とバージョンは `technology-stack.md` が持つ。ここは件数・方針・向きだけを持つ。

## 依存の数え方と数

数値は測定範囲とセットでないと意味が変わる。3 つの数は違うものを数えている。

| 数                         | 値  | 測定範囲                                                                                                                                                                                           |
| -------------------------- | --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 直接依存                   | 26  | `Cargo.lock` の `[[package]] name = "scout"` ブロックの `dependencies` 配列。`Cargo.toml` の `[dependencies]` 23 + `[dev-dependencies]` 4 から、両方に現れる `tokio` の重複 1 を引いた数と一致する |
| 推移含む総数               | 311 | `Cargo.lock` の `^name = ` 行                                                                                                                                                                      |
| version が割れている crate | 11  | 同名で複数 version が `Cargo.lock` に並ぶもの                                                                                                                                                      |

**依存を crate 間で持たない。** Cargo workspace ではなく単一 crate なので、内部パッケージ依存は存在しない。`Cargo.lock` の `[[package]] name = "scout"` は 1 件である。

## ライセンスとソースの方針

`deny.toml` が定める。**deny リストを持たず、許可リストに列挙しないことで strong copyleft を拒否する** 設計である。

| 設定                         | 値                                                                                                                                                                 |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `[licenses] allow`           | 13 件: MIT、Apache-2.0、Apache-2.0 WITH LLVM-exception、BSD-2-Clause、BSD-3-Clause、ISC、0BSD、Zlib、Unicode-3.0、CDLA-Permissive-2.0、BSL-1.0、MPL-2.0、Unlicense |
| `[licenses] deny`            | 無し                                                                                                                                                               |
| `[bans] multiple-versions`   | `"warn"`                                                                                                                                                           |
| `[sources] unknown-registry` | `"deny"`                                                                                                                                                           |
| `[sources] unknown-git`      | `"deny"`                                                                                                                                                           |
| `[sources] allow-registry`   | crates.io のみ                                                                                                                                                     |

**許可リストは 13 件である。** attempt 1 の先行資料が同じリストを一方で「13 件」、他方で「14 件で明示」と書き、列挙そのものは 13 件だった。`deny.toml` の `[licenses] allow` 配列を直接数え直して 13 件で確定した。その資料は attempt 2 のスキャン報告に置き換わっており、現在 `<record>/inception/reverse-engineering/developer-scan.md` にある内容はライセンスに触れていない。**再走査でどの資料から引く場合でも 14 を引き継がないこと。**

検査は CI の security job が `cargo deny check` で毎回走らせる。実行タイミングと他 2 ツールの並びは `code-quality-assessment.md` の `## CI/CD` が持つ。

## version 分裂

`multiple-versions = "warn"` なので、以下 11 件はビルドを落とさず警告に留まる。

`base64`、`core-foundation`、`cpufeatures`、`getrandom`、`html5ever`、`markup5ever`、`r-efi`、`rand_core`、`rand`、`syn`、`windows-sys`

`base64` の分裂は用途が分かれている — scout が直接使う 0.23.1 と、推移的に入る 0.22.1 が並ぶ。

## 外部プロセスとランタイム依存

crate だけが依存ではない。実行時に外部のバイナリとネットワークサービスへ依存する箇所が 3 つある。

| 依存先                              | 必須か                        | 不在時の振る舞い                                                                                                                                                        |
| ----------------------------------- | ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `gh` CLI (`gh auth token`)          | 任意                          | `GITHUB_TOKEN` / `GH_TOKEN` が無いときの最後の解決手段。`TOKEN_RESOLVE_TIMEOUT` でタイムアウトし、stderr は破棄して終了コードだけ報告する (DR-0018)                     |
| chromium バイナリ                   | `js-rendering` feature 時のみ | `src/fetch/cdp/launch.rs` の探索テーブルが macOS と非 macOS で分かれる。CI は `--run-ignored all` で必ず走らせるので、chromium 不在のランナーは skip ではなく fail する |
| Brave / GitHub / Slack の各 Web API | サブコマンドごと              | API キー未設定はエラーとして envelope に載る                                                                                                                            |

外部サービスのエンドポイント一覧は `api-documentation.md` の `## Consumed — 外部 API` が持つ。

## crate 内のモジュール依存

向きは一方向で、循環は無い。

```
main -> lib -> tools -> backends -> cross-cutting leaves
                          fetch
                          github
                          slack
                          brave
                          search
```

<!-- Text fallback: main depends on lib, lib on tools, tools on the five backend modules, and every backend on the cross-cutting leaves; no leaf imports a backend and no backend imports tools. -->

`src/tools.rs` は `brave::client`/`clock`/`envelope`/`fetch`/`github`/`markdown`/`rng`/`slack`/`token_source`/`yaml` を直接 import し、逆向きの import を持たない。`search` だけが例外的に他のバックエンド 2 つ (`brave`、`fetch`) を合成する。

この向きは `src/tools.rs` と `src/fetch.rs` の `use crate::…` を読んで確認した範囲であり、全ファイルの import を機械的に検証したものではない。図と各コンポーネントの依存先は `architecture.md` の `## コンポーネント関係` と `component-inventory.md` が持つ。

## 依存の自動更新

`renovate.json` はリポジトリ直下の 33 行で、`$schema`/`extends`/`customManagers` (1 要素)/`packageRules` (3 要素) からなる。

**上書き 1 — 規則は 3 つではなく 4 つである。** attempt 1 は 3 規則を記録していたが、`matchDepNames: ["rust"]` の packageRule を数え落としていた。**上書き 2 — このファイルだけでは実効設定が決まらない。** `"extends": ["github>thkt/renovate-config"]` が共有プリセットを取り込むが、そのプリセットは取得していない。**以下で確定したのは「ローカルに書かれた 4 規則の中身」であって、renovate が最終的に適用する設定の全体ではない。** この境界は `code-quality-assessment.md` の `## 未確認の項目` にも残してある。

| 規則                           | 種別             | 内容                                                                                                                                                                                                     |
| ------------------------------ | ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| MSRV 追跡                      | `customManagers` | `customType: regex`。`Cargo.toml` から `rust-version` を名前付きキャプチャ `currentValue` で拾い、`depNameTemplate: rust` / `datasourceTemplate: docker` / `versioningTemplate: semver-coerced` を与える |
| `rust` へのラベル付け          | `packageRules`   | `matchDepNames: ["rust"]` に `addLabels: ["dependencies", "rust"]` と `commitMessageTopic: "rust-version (MSRV)"` を付ける。バージョン制約は持たない                                                     |
| `htmd` + `markup5ever_rcdom`   | `packageRules`   | `matchManagers: ["cargo"]`、`matchDepNames: ["htmd", "markup5ever_rcdom"]`、`groupName: "htmd + markup5ever_rcdom"`                                                                                      |
| `markup5ever_rcdom` の上限固定 | `packageRules`   | `matchManagers: ["cargo"]`、`matchDepNames: ["markup5ever_rcdom"]`、`allowedVersions: "<0.39"`                                                                                                           |

**MSRV の datasource が `docker` である点は非自明である。** Rust の MSRV を crates.io ではなく Docker Hub の `rust` イメージのタグ一覧で追う。`semver-coerced` は 2 桁表記を semver へ寄せるための指定である。**この manager は空振りしていない** — `Cargo.toml` の `[package]` に `rust-version` が実在して `matchStrings` の正規表現に当たるので、規則は依存 `rust` を生成し、ラベル付けの規則もその依存に対して発火する。

**グループ化と上限固定は別々に要る。** グループ化の `description` は「バージョン分裂がコンパイルエラーではなくレビュー可能な更新として届くよう、両 crate を 1 つの PR に載せる」と書く。上限固定の `description` は **グループ化だけでは足りない理由** を書く —「renovate は同時に発生した更新を 1 つの PR にまとめるが、更新がある方の crate については単独でも PR を開く。その単独の bump はコンパイルできない」。

**相互参照は両方向で一致している。** 上限固定の `description` は `Cargo.toml` の `markup5ever_rcdom` の pin コメントを指し、そのコメントは `renovate.json` の `allowedVersions: "<0.39"` を指す。値も `<0.39` と `"0.38"` で整合しており、古い相互参照ではない。**`Cargo.toml` 側だけが持つ情報が 1 つある** — 解除手順である。htmd が動いたら pin の引き上げと `allowedVersions` の除去を同時にやること、さもないと renovate はこの crate の更新を一切提示しなくなる、と書かれている。

`markup5ever_rcdom` を pin する理由自体は、htmd が再エクスポートしない `NodeData` を読むために htmd と同じ crate へ解決させる必要があるためである (`technology-stack.md` の本番依存表)。
