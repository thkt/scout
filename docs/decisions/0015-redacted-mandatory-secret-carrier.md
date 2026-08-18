---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# Redacted Mandatory Secret Carrier

## Context and Problem Statement

scout は GitHub token・Slack token・Brave Search API key の 3 種の secret を扱う。CLI はクラウド上の AI エージェントから継続的に実行され、log 集約が標準である。secret を平文の `String` で構造体に持つと、`#[derive(Debug)]` 構造体の debug 出力、tracing の構造化 log、panic backtrace、serialize 経由で token が silent に漏れうる。

scout は `Redacted` newtype を導入し、全 secret をこの型で運ぶことで上記の漏洩面を型レベルで塞いでいる。しかしこの「secret は必ず Redacted に包む」という方針と、その保証範囲・限界が ADR として記録されていない。

## Decision Drivers

- クラウド実行 + log 集約環境では debug/log への secret 混入が現実的な漏洩経路
- env var 読み取り境界で必ず包めば、以降の構造体は derive Debug でも安全になる
- 個人 OSS scale で、規律 (慣習) より型強制の方が漏洩を確実に防げる

## Considered Options

- Option A: secret を必ず通す newtype `Redacted` を境界で構築する (採用)
- Option B: 規律のみ。secret は平文 String、Debug 派生を避ける慣習で運用
- Option C: `secrecy` / `zeroize` 等の外部 crate を採用する

## Decision Outcome

Chosen option: Option A。`Redacted` は内部 `String` を隠す newtype で、`Debug` impl は内容を出さず `[REDACTED]` のみを書く。構築は `Redacted::new(&str) -> Option<Self>` と `Redacted::from_env_var` の 2 つで、どちらも trim 後 empty/whitespace を弾く。前者は `None`、後者は caller の error 型を返す。平文取得は明示的な `expose(&self) -> &str` だけが許す。`Display`・`Deref`・`Serialize` は実装せず、暗黙の文字列化・serialize を型で禁じる。env var 読み取り (token_source / slack client / brave client) の境界で必ず `Redacted::new` を通す。

Option B は人間の規律に依存し、1 箇所の忘れが漏洩に直結するため却下。Option C は依存追加に対し scout の要件 (Debug マスク + 構築時 non-empty 検証) が自前 newtype で十分満たせるため YAGNI で却下。`zeroize` の drop-time 消去は脅威モデル (log 漏洩) に対する追加価値が薄い。

### Consequences

- Good, because env var 境界で `Option` を返すため「secret 未設定」を明示的に扱わせる
- Good, because `Redacted` を含む構造体の derive Debug が `[REDACTED]` を出し、偶発的な log 漏洩を塞ぐ。ただし `Debug` を手書きする構造体には及ばない。`src/brave/client.rs` の `BraveClient` は `api_key` に `"<redacted>"` の literal を書いており、`Redacted` の `Debug` を呼ばない。この構造体へ秘密の field を足す人は、同じ手当てを自分で書く必要がある
- Good, because newtype はゼロコストで、`expose()` 明示呼び出しがないと平文へ到達できない
- Bad, because `Serialize` は未実装だが、誤って `.expose()` した値を serde に渡せば保護は効かない
- Bad, because `Clone` は secret をメモリに複製し、drop 時の消去 (zeroize) は無い
- Bad, because 境界での包み忘れは型で検出できず、平文 String のまま運ぶ経路を作れる (現状は全境界が `Redacted::new` 経由)
- Neutral, because `Redacted::new` は汎用 secret carrier のため token 種別を知らず `xoxp-` prefix を検証しない。prefix 要求自体は `SlackClient::from_env_with` の構築時検証 (`SlackError::TokenWrongType`) で強制済み (ADR-0022)

### Confirmation

`src/redacted.rs` のユニットテスト `[T-RD001..T-RD004]` が、`format!("{:?}")` が `[REDACTED]` になること・empty/whitespace 構築が `None` になること・`expose()` が trim 済み内容を返すことを assert する。token 源との結合は `src/slack/client/constructor_tests.rs` の `[T-SK033..T-SK035]` と GitHub の `[T-GH018/T-GH019]` が、注入された source から token が構築されること・未設定で `None` になることを検証する。`Display`/`Serialize` は impl が存在しないため negative test は無い (型の不在が保証)。

## Pros and Cons of the Options

### Option A: 必須 newtype `Redacted` (採用)

secret を境界で newtype に包み、Debug マスク + 構築時検証 + expose 明示を強制する。

- Good, because 型レベルで debug/log 漏洩を塞ぐ
- Good, because 依存追加なしで要件を満たす
- Bad, because 境界での包み忘れは型では防げない

### Option B: 規律のみ

平文 String + Debug 派生回避の慣習。

- Good, because コードが最も単純
- Bad, because 1 箇所の忘れが漏洩に直結し、レビュー依存

### Option C: 外部 crate (secrecy / zeroize)

既製の secret 型を使う。

- Good, because drop-time zeroize 等の追加保証
- Bad, because 依存追加に見合う追加価値が脅威モデル上薄い

## More Information

### 型定義 (一次ソース `src/redacted.rs` の `Redacted`)

- `pub(crate) struct Redacted(String)` — `derive(Clone)` のみ
- `pub fn new(s: &str) -> Option<Self>` — trim 後 empty なら `None`、非空なら `Some(trim 済み)`
- `pub fn expose(&self) -> &str` — 平文取得の唯一の手段
- `impl fmt::Debug` — `f.write_str("[REDACTED]")`
- `Display` / `Deref` / `Serialize` は未実装

### 包み込みサイト

| secret        | 場所                                           | env var                                       |
| ------------- | ---------------------------------------------- | --------------------------------------------- |
| GitHub token  | src/token_source.rs (`resolve_from_env_or_gh`) | `GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token` |
| Slack token   | src/slack/client.rs (`from_env_with`)          | `SLACK_TOKEN`                                 |
| Brave API key | src/brave/client.rs (`from_env_with`)          | `BRAVE_SEARCH_API_KEY`                        |

### 参照

- `src/redacted.rs` の `Redacted` と `mod tests` (型定義 + テスト)
- GitHub token 解決の詳細は別 ADR (token precedence + stderr drop)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 #3。`xoxp-` prefix の error 文言不一致は横流し BUG 事項)
