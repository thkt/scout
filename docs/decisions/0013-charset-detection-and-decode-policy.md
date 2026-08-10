---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# Charset Detection and Decode Policy

## Context and Problem Statement

scout は外部バイト列を 2 つの経路で Unicode テキストへ復号する。fetch 経路 (`src/fetch/download.rs`) は任意の Web ページを取得し、GitHub 経路 (`src/github/encoding.rs`) は repo 内ファイル本文を取得する。どちらも server や file が宣言する charset が誤っている・欠落している場合があり、誤った復号は AI エージェントの context に mojibake (文字化け) を silent に流し込む。

2 経路は失敗許容度が逆である。fetch 経路は誤ラベルの Web ページでも本文を返さないとエージェントの調査が止まるため best-effort 復号 + degraded 信号で exit 0 する。GitHub 経路はユーザーが `--encoding` を指定して再試行できるため、復号不能を hard error で表面化する方が UX が良い。両者は単バイト encoding の検出信頼性という共通問題を抱え、検出ゲートを `src/charset.rs` に共有する。

この復号方針は実装に埋め込まれているが、判断 (なぜ label-first か、なぜ単バイト検出を捨てるか、なぜ 2 経路で失敗挙動を変えるか) が ADR として記録されていない。さらに GitHub 経路のコメントは `BR-001/002/003`・`FR-007/008` という決定コードを参照するが、その定義文書がリポジトリに存在しない。

## Decision Drivers

- AI エージェント consumer は誤ラベル本文でも一次ソースを読み続けたい (fetch 経路)。中断より degraded 継続が outcome に資する
- 単バイト encoding (windows-1252, iso-8859-\*) は任意バイトを errors なく受理するため、`had_errors == false` が検出信頼性の信号にならない
- GitHub ファイル読みは対話的に `--encoding` 再試行できるため、誤復号を黙認するより hard error で表面化する方が良い
- 同一の検出信頼性ロジックを 2 経路で重複させない (DRY)

## Considered Options

- Option A: label/hint を起点に、信頼できる多バイト検出のみ fallback し、最終手段を経路ごとに変える (採用)
- Option B: 常に chardetng 検出を先に行い label を無視する
- Option C: label のみで復号し、errors があれば常に hard error にする

## Decision Outcome

Chosen option: Option A。両経路とも宣言エンコーディング (fetch は HTTP `Content-Type` charset、GitHub は `--encoding` hint または BOM) を起点に復号し、失敗時のみ信頼ゲート付き chardetng 検出へ落とす。検出ゲートは `src/charset.rs:is_reliable_detection` に共有し、多バイト 8 種 (UTF-8, Shift_JIS, EUC-JP, ISO-2022-JP, Big5, GBK, GB18030, EUC-KR) のみ信頼する。最終手段は経路で分岐する。fetch は lossy UTF-8 + `uncertain = true` を返し exit 0、GitHub は `NonUtf8` error + 再試行ヒントで fail する。

Option B は server/user の宣言意図を捨て、正しい label を無視するため却下。Option C は誤ラベルされた多バイト本文 (例: Shift_JIS を UTF-8 と宣言) を常に失敗させ、fetch のリカバリを潰すため却下。単バイト誤ラベルで偶然 valid UTF-8 になるバイト列は検出不能であり、これは既知の non-goal として受容する。

### Consequences

- Good, because 正しい label は尊重され、誤ラベルの多バイト本文は検出で自動リカバリされる (test T-F068, T-GE004)
- Good, because 単バイト mojibake (windows-1252 smart quotes を UTF-8 と宣言) は信頼ゲートで弾かれ、fetch は `uncertain`、GitHub は error になる (test T-F063)
- Good, because 検出ゲートが 1 箇所に集約され、2 経路の重複を排除する
- Bad, because 単バイト誤ラベルで偶然 valid UTF-8 になる本文は検出できず silent に誤グリフを返す (既知 non-goal)
- Bad, because fetch 経路は `<meta charset>` を見ず HTTP header のみを信頼するため、header が誤り BOM も無い場合は chardetng のみがリカバリ手段になる
- Bad, because GitHub 経路コメントの `BR-/FR-` コードは本 ADR が決定の根拠を与えるまで宙吊りだった (本 ADR で解消、source コメントの再ポイントは別タスク)

### Confirmation

fetch 経路は `src/fetch/download/charset_tests.rs` が label 尊重・多バイトリカバリ・単バイト uncertain を網羅する (T-F063, T-F068 系)。GitHub 経路は `src/github/encoding/tests.rs` が explicit hint・BOM 優先・chardetng 先行・binary 弾き・hard error を網羅する。`[T-GE015]` は BOM が宣言したエンコーディングで復号できない本文を error にすること、`[T-GE016]` は正常な BOM 経路が変わらないことを assert する。この 2 件が入るまで `decode_bom` だけが `had_errors` を捨てて置換文字入りの本文を `DetectionSource::Bom` で返しており、最も弱い宣言 (ファイル内の BOM) が最も寛容という逆転が残っていた。共有ゲートの 8 種 whitelist は `src/charset.rs` の定義が単一の真実源で、同ファイルの `[T-CS001..T-CS003]` が直接 pin する。両経路のテストは間接的にしか触れず、実際 `ISO_2022_JP` / `BIG5` / `GB18030` はどちらの経路のテストにも現れなかったため、whitelist から落としても 1 件も失敗しない状態だった。chardetng / encoding_rs を更新する際は、`encoding.decode` が BOM を honor する挙動 (fetch 経路の前提) と `decode_without_bom_handling` の挙動 (GitHub 経路の前提) を再検証する。

## Pros and Cons of the Options

### Option A: label/hint 起点 + 信頼ゲート付き検出 fallback + 経路別の最終手段 (採用)

宣言エンコーディングを起点に復号し、失敗時のみ多バイト限定検出へ落とし、最終手段を fetch (lossy+uncertain) と GitHub (hard error) で分岐する。

- Good, because 宣言意図を尊重しつつ誤ラベルの多バイトをリカバリする
- Good, because 経路ごとの失敗許容度 (継続 vs 表面化) に最適化する
- Bad, because 経路で挙動が分岐し、復号失敗時の振る舞いを 1 文で言えない

### Option B: 常に chardetng 検出を先に行う

label を無視し検出結果を優先する。

- Good, because 誤ラベルの多バイトリカバリは単純化する
- Bad, because 正しい label を無視し、server/user の agency を奪う
- Bad, because 単バイト検出の誤りが silent な mojibake を生む

### Option C: label のみ、errors で常に hard error

宣言エンコーディングで復号し、errors があれば検出せず失敗する。

- Good, because 挙動が単純で予測可能
- Bad, because 誤ラベルの多バイト (Shift_JIS を UTF-8 宣言) を常に失敗させ fetch を不必要に壊す
- Bad, because fetch 経路のエージェント継続性 outcome に反する

## More Information

### 復号フロー (一次ソース)

fetch 経路 `decode_body` (src/fetch/download.rs:137-174):

| ステップ | 処理                                                       | 結果                                                     |
| -------- | ---------------------------------------------------------- | -------------------------------------------------------- |
| 1        | `Content-Type` charset (既定 `utf-8`) で `encoding.decode` | `had_errors == false` なら `uncertain: false` で返す     |
| 2        | lossy または unknown label なら `detect_decode`            | 多バイト且つ clean decode なら `uncertain: false` で返す |
| 3        | どちらも失敗                                               | `String::from_utf8_lossy` + `uncertain: true`、exit 0    |

GitHub 経路 `decode_bytes` (src/github/encoding.rs:60-161):

| 優先 | 処理                                                                 | source      |
| ---- | -------------------------------------------------------------------- | ----------- |
| 1    | `--encoding` hint があれば `decode_explicit`、失敗は `NonUtf8` error | Explicit    |
| 2    | BOM があれば `decode_bom`、失敗は `NonUtf8` error                     | Bom         |
| 3    | null byte を含めば binary として `NonUtf8` error                     | —           |
| 4    | 信頼ゲート通過の chardetng が clean decode                           | Detected    |
| 5    | strict UTF-8 検証 (実質到達不能の defensive backstop)                | AssumedUtf8 |
| 6    | 全失敗で `NonUtf8` error + best-guess 再試行ヒント                   | —           |

共有ゲート `is_reliable_detection` (src/charset.rs:8-20): UTF-8, Shift_JIS, EUC-JP, ISO-2022-JP, Big5, GBK, GB18030, EUC-KR の 8 種のみ `true`。

### 使用 crate

- `encoding_rs` (Cargo.toml): `Encoding::for_label` / `for_bom`、`decode` (BOM honor)、`decode_without_bom_handling`
- `chardetng`: `EncodingDetector::new(Iso2022JpDetection::Allow)` + `feed(bytes, true)` + `guess(None, Utf8Detection::Allow)`

### 既知の non-goal

単バイト誤ラベルで偶然 valid UTF-8 バイト列になる本文は、検出不能なため誤グリフを silent に返す。検出可能な mojibake (smart quotes, em dash 等) は信頼ゲートで弾かれるが、UTF-8 と一致するバイト列は区別できない。fetch 経路の `uncertain` フラグも GitHub 経路の error も、この境界を越えるケースには発火しない。

### 参照

- `src/charset.rs:8-20` (共有検出ゲート)
- `src/fetch/download.rs:103-193` (fetch 経路 `extract_charset` / `decode_body` / `detect_decode`)
- `src/github/encoding.rs:52-161` (GitHub 経路 `decode_bytes` 一式)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 #1)
- GitHub 経路コメントの `BR-001/002/003`・`FR-007/008` コードは本 ADR が決定根拠を提供する。source コメントの参照差し替えは別タスクで追跡する
