---
status: "accepted"
date: 2026-05-13
decision-makers: thkt (project owner)
---

# Adopt sysexits.h Exit Code Convention for CLI

## Context and Problem Statement

scout は CLI で他 tool / shell script から呼ばれる前提があり、exit code は公開 API の一部となる。エラー時に 0/1 だけだと CLI script が「retry すべき」「設定が間違ってる」「アクセス不能」を区別できず、運用判断ができない。業界には複数の exit code 規約があり、どれを採用するかで CLI script の互換性と将来の変更可能性が決まる。

## Decision Drivers

* CLI script からの利用で retry / fail-fast の判断 path が必要
* exit code は公開 API であり、互換性破壊は downstream 全体に伝播する
* 個人 OSS 規模で覚えやすく、調査コストの低い規約が望ましい

## Considered Options

* Option A: sysexits.h (OpenBSD/BSD-derived) 規約
* Option B: 独自スキーマ
* Option C: 0/1 のみ (POSIX 最小)

## Decision Outcome

Chosen option: Option A (sysexits.h), because POSIX 慣習として浸透しており、CLI script 側で `man sysexits` 一発で意図が読める。独自スキーマ (Option B) は学習コストを scout uniform にし、0/1 のみ (Option C) は retry/fail-fast 判断ができない。

採用する code:

| Code | Name        | scout での意味                                |
| ---- | ----------- | --------------------------------------------- |
| 0    | EX_OK       | 成功                                          |
| 64   | EX_USAGE    | コマンドラインの使い方が誤り                  |
| 65   | EX_DATAERR  | 入力データが invalid (URL invalid 等)         |
| 66   | EX_NOINPUT  | 入力ファイルやリソースが見つからない          |
| 74   | EX_IOERR    | I/O エラー (network, disk)                    |
| 75   | EX_TEMPFAIL | 一時的失敗 (rate limit, transient API error)  |

### Consequences

* Good, because retry 判断 (EX_TEMPFAIL) が CLI script で `[ $? -eq 75 ] && retry` のように書ける
* Good, because `sysexits.h` という公開ドキュメントで規約意図が共有される
* Good, because lib.rs T-H000 test が --help に exit code 一覧と sysexits.h ref を含めることを enforce
* Bad, because Windows / 非 POSIX 環境では命名が馴染まない

### Confirmation

`src/lib.rs:220-254` の T-H000 test が以下を check:

* `--help` 出力に "Exit codes" section が存在
* "sysexits.h" の文字列を含む
* 採用した全 code (64, 65, 66, 74, 75) が `--help` に列挙
* "Usage error", "Temporary failure" 等の説明文字列を含む

新規 exit code 追加時は本 test を update する。test 削除は本 ADR を supersede してから行う。

## Pros and Cons of the Options

### Option A: sysexits.h 規約 (採用)

OpenBSD/FreeBSD の `<sysexits.h>` で定義されている標準 code。

* Good, because POSIX 系で標準的、`man sysexits` で意図が読める
* Good, because retry 判断 (EX_TEMPFAIL) が確立されている
* Good, because Rust の `process::exit(75)` で簡潔に書ける
* Bad, because Windows / 非 POSIX で命名が馴染まない
* Bad, because sysexits.h は POSIX 標準ではなく BSD-derived (Linux glibc は同等の `<sysexits.h>` を提供するが標準準拠ではない)

### Option B: 独自スキーマ

scout 独自の意味づけ (例: 10 = network, 20 = parse, 30 = auth)。

* Good, because scout のドメインに合わせて意味設計できる
* Bad, because CLI script 側で `man scout-exitcodes` 等の追加調査が必要
* Bad, because 業界知識を活かせない

### Option C: 0/1 のみ (POSIX 最小)

成功 = 0、失敗 = 1。

* Good, because 最小コスト、覚えることなし
* Bad, because retry / fail-fast 判断ができない
* Bad, because diagnostic 価値が低い

## More Information

### 採用 code 詳細

| Code | 利用箇所 (例)                       |
| ---- | ----------------------------------- |
| 64   | clap parse 失敗                     |
| 65   | URL parse 失敗、JSON parse 失敗     |
| 66   | repo not found、page 404            |
| 74   | network error、TLS error            |
| 75   | rate limit、Gemini API 503          |

### Reassessment Triggers

| Trigger                                      | アクション                                    |
| -------------------------------------------- | --------------------------------------------- |
| Windows 環境がサポート対象に加わる           | exit code 規約を再評価 (Windows convention 検討) |
| 新規 error mode で 6 つの code に収まらない | code 追加 + T-H000 test update                |
| sysexits.h 互換性違反 incident 発生          | 本 ADR supersede + 移行 ADR 起票              |

### 参照

* `<sysexits.h>` man page: https://man.openbsd.org/sysexits
* `src/lib.rs:220-254` (T-H000 enforcement test)
* `README.ja.md:232` ("sysexits.h 規約に準拠")
* `docs/audit/2026-05-13-undocumented-decisions.md` (本 ADR の根拠 audit)
