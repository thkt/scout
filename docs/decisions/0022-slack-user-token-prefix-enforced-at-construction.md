---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# Slack User-Token Prefix Enforced at Construction

## Context and Problem Statement

scout は Slack URL の解決に `SLACK_TOKEN` を使う。`TokenNotSet` のヒントは User OAuth token (`xoxp-…`) を要求すると約束していたが、トークンを運ぶ `Redacted::new` は空・空白のみを弾くだけで prefix を検証しない (ADR-0015)。このため bot token (`xoxb-…`)、app-level token (`xapp-…`)、workflow token (`xwfp-…`)、あるいは任意の文字列が `SlackClient::from_env_with` の構築を通過し、最初の API 呼び出しまで失敗が遅延する。そこで返るのは `invalid_auth` 等の不透明な Slack API エラーで、エージェントは「自分が `SLACK_TOKEN` に渡した値が User token ではない」という根本原因に到達しにくい。エラー文の契約 (xoxp- を要求) と実装 (任意の非空文字列を受理) が乖離していた (issue #261)。

`docs/audit/2026-06-24-020601-adr-gaps.md` の candidate 7 はこれを「契約ではなくバグ」と分類し ADR を drop した。本 ADR は、その修正で選んだ「どこで検証し・どの prefix を要求し・どの exit code を返すか」という、修正後に残る挙動契約を記録する。

## Decision Drivers

- エラー文が約束する契約 (xoxp- User token) と実装の受理範囲を一致させる
- 誤ったトークン種別は network I/O 前 (構築時) に失敗させ、不透明な API エラーへの遅延を断つ (ADR-0019 の fail-fast 方針と同型)
- エージェントが exit code とヒントだけで自己修正できる UsageError (exit 64) に揃える
- `unsafe_code = "forbid"` 下で `env::set_var` を使わずトークン分岐をテストできる注入接ぎ目を保つ (ADR-0007/0008)

## Considered Options

- Option 1: `from_env_with` で `xoxp-` prefix を検証し、不一致を新 `SlackError::TokenWrongType` (UsageError, exit 64) で構築時に拒否する (採用)
- Option 2: エラー文から `xoxp-` の含意を外し、契約を弱める (検証は追加しない)

## Decision Outcome

Chosen option: Option 1。`SlackClient::from_env_with` は `SLACK_TOKEN` を読み、`Redacted::new` 通過後に `USER_TOKEN_PREFIX` (`"xoxp-"`) で始まるかを検証する。始まらなければ新 variant `SlackError::TokenWrongType` を返す。`classify()` はこれを `TokenNotSet` と同じ `ErrorCode::UsageError` + 「Export a User OAuth token to SLACK_TOKEN (xoxp-…)」ヒントに割り当て、`From<SlackError> for ScoutError` が `classify()` 経由で exit 64 (ADR-0002) へ自動的に route する。許可 prefix は単一の `xoxp-` のみ — Slack の token taxonomy で user token はこの prefix のみ、bot (`xoxb-`)、app-level (`xapp-`)、workflow (`xwfp-`)、config/service token はいずれも別 prefix を持つ (<https://api.slack.com/concepts/token-types#user> で確認、2026-06)。scout が解決する channel・thread・user は人間自身の workspace 可視範囲と一致する必要があり、bot token は app が追加された channel しか見えないため、user token のみを許可する。

Option 2 は false な契約を消すだけで、誤ったトークン種別が依然構築を通過し API エラーへ遅延する根本原因を残すため却下。Option 1 のコストは、将来 scout が bot scope に対応した場合に正当な `xoxb-` 構成も一律拒否する点だが、現状の endpoint は user scope 前提のため許容する。

### Consequences

- Good, because 誤ったトークン種別が構築時に変数名つき exit 64 で失敗し、エージェントが API エラーを待たず即自己修正できる (T-SK065/066)
- Good, because エラー文の契約 (xoxp-) と実装の受理範囲が一致し、ヒント・`--help`・実装が同じ真実源を指す (`src/lib.rs` の `after_help`)
- Good, because 新 variant は `classify()` 経由で exit code に自動 route し、手動の exit-code 配線が不要 (ADR-0011, T-SLC011, T-ER001a)
- Good, because 検証は `from_env_with` の注入接ぎ目で `env::set_var` なしにテストでき、`unsafe_code = "forbid"` を保つ (ADR-0007/0008)
- Bad, because 将来 bot scope に対応した場合、正当な `xoxb-` 構成も拒否され、許可 prefix のリスト化にコード修正が要る
- Bad, because prefix の一致は token の有効性を保証しない (`xoxp-garbage` は構築を通過し API で `invalid_auth` になる) — 検証は種別の早期弾きに限られる

### Confirmation

`src/slack/client/constructor_tests.rs` と `src/slack/classify_tests.rs`、`src/tools/errors/exit_code_tests.rs` のテストが契約を pin する。`[T-SK065]` は bot token (`xoxb-…`) が `TokenWrongType` で拒否されること、`[T-SK066]` は任意の非 `xoxp-` 文字列が同じく拒否されること、`[T-SK035]` は `xoxp-` token が `Ok(client)` を返し token が `Redacted::expose()` を round-trip することを assert する。`[T-SLC011]` は `TokenWrongType` が `UsageError` + `SLACK_TOKEN` ヒントに分類されること、`[T-ER001a]` は exit 64 になることを assert する。`[T-SK033/034]` は既存の `TokenNotSet` 経路 (未設定・空白のみ) が prefix 検証より前に変わらず弾かれることを守る。

## Pros and Cons of the Options

### Option 1: 構築時に prefix 検証 + TokenWrongType (採用)

`from_env_with` で `xoxp-` を検証し、不一致を UsageError で拒否する。

- Good, because 契約と実装が一致し、誤ったトークン種別を network I/O 前に弾く
- Good, because exit 64 + ヒントでエージェントが自己修正できる
- Bad, because bot scope 対応時に許可 prefix の拡張がコード修正を要する

### Option 2: エラー文を弱める

`xoxp-` の含意を外し、検証は追加しない。

- Good, because 実装コストが最小で、false な契約は消える
- Bad, because 誤ったトークン種別が依然構築を通過し、不透明な API エラーへ遅延する根本原因が残る

## More Information

### 検証経路 (一次ソース `src/slack/client.rs` の `from_env_with`)

```
raw   = get_var("SLACK_TOKEN")          // 未設定/NotUnicode → TokenNotSet
token = Redacted::new(&raw)             // 空・空白のみ      → TokenNotSet (ADR-0015)
if !token.expose().starts_with("xoxp-") // 非 user token     → TokenWrongType (本 ADR)
```

`TokenNotSet` (未設定・空) と `TokenWrongType` (種別違い) は別 variant だが、`classify()` で同じ UsageError + 同一ヒントに畳まれる (`src/slack.rs` の `classify`) — どちらも「人間が修正すべき誤設定」で caller-facing の扱いが同じため。

### Slack token taxonomy (api.slack.com/concepts/token-types, 2026-06)

| 種別      | prefix  | scout が許可 |
| --------- | ------- | ------------ |
| user      | `xoxp-` | ✓            |
| bot       | `xoxb-` | ✗            |
| app-level | `xapp-` | ✗            |
| workflow  | `xwfp-` | ✗            |

service/config token は長命の user token として `xoxp-` を共有するため許可側に入る。

### candidate 7 (gap audit) の吸収

`docs/audit/2026-06-24-020601-adr-gaps.md` は本件を「bug → fix prefix check or correct error text」として ADR を drop した。fix で Option 1 を選んだ結果、検証箇所・許可 prefix・exit code の組が修正後も残る挙動契約となったため、本 ADR に記録して candidate 7 を解決済みとする。

### 参照

- `src/slack/client.rs` の `USER_TOKEN_PREFIX` 定数と根拠コメント、および `from_env_with` の検証
- `src/slack.rs` の `TokenNotSet` / `TokenWrongType` variant と `classify`
- `src/lib.rs` の `after_help` (`--help` の `SLACK_TOKEN` 表記)
- ADR-0002 (sysexits。UsageError = exit 64)
- ADR-0003 / ADR-0011 (classification 契約と優先度。新 variant の自動 route)
- ADR-0007 (`from_env_with` 注入 factory。Brave と同型)
- ADR-0015 (Redacted secret carrier。prefix 未検証の sibling)
- ADR-0019 (env 検証の fail-fast 方針)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (candidate 7、本 ADR が解決)
