# AI-DLC Workflows 2.0 導入記録

対象: scout main
実施日: 2026-08-28
基準 commit: c43278f (v2.6.0)

scout の開発フローを AI-DLC Workflows 2.0 へ置き換えるために、engine を `.claude/` へ、workspace を `aidlc/` へ入れた。同梱の設定のうち 3 点を落として入れている。再インストールと撤去に必要な情報をここに残す。`/think` `/issue` `/build` `/audit` などの旧フローは `~/.claude` のグローバル資産なので、この導入では触っていない。

## 固定した upstream

| 項目 | 値 |
| ---- | -- |
| リポジトリ | https://github.com/awslabs/aidlc-workflows |
| ブランチ | `v2` |
| commit | `2fbee12fb29d2a6614b70b6f61f3cceeaf235245` (2026-08-28) |
| AIDLC_VERSION | 2.6.123 |

`v2` はリリースタグではなくブランチである。GitHub Releases は v1.0.1 (2026-06-30) で止まっており、PR #756 の six-command reshape が進行中。再現するときは `git clone --branch v2` の後に上の commit を checkout する。

## 入れたもの

| 配置元 | 配置先 | 中身 |
| ------ | ------ | ---- |
| `dist/claude/.claude/.` | `.claude/` | skills 42、agents 14、hooks 18、tools、knowledge、aidlc-common、scopes 11、sensors 6、`CLAUDE.md`、`rules/aidlc.md`、`settings.json` |
| `dist/claude/aidlc/.` | `aidlc/` | `spaces/default/memory/` の method tree。`/aidlc --doctor` の workspace shell ready がこれを見る |

`.claude/` は scout の `.gitignore` が丸ごと無視している。scout の `.claude/rules/CONVENTIONS.md` `.claude/rules/CORRECTIONS.md` `.claude/OUTCOME.md` `.claude/agent-memory/` も元から untracked なので、engine を untracked のまま置くのはこのリポジトリの既存の扱いに揃う。AI-DLC が持ち込む `rules/aidlc.md` は既存 2 本とファイル名が衝突しない。

`aidlc/` は upstream 同梱 `.gitignore` の分割方針に従って git に載せる。per-user cursor (`aidlc/active-space`, `aidlc/spaces/*/intents/active-intent`) と machine-local runtime は無視し、method memory・intents.json・aidlc-state.md・audit shard・artifacts は commit する。同梱 `.gitignore` のうち Vite/Node 向けの boilerplate (`node_modules` `dist` `*.log` `.vscode/*` ほか) は取り込まず、`# AI-DLC` 以降だけを scout の `.gitignore` へ足した。`.claude/settings.local.json` の 1 行も、scout が `.claude/` を丸ごと無視するので落とした。

upstream issue #937 は `aidlc-state.md` が machine-local な絶対パスを共有 state へ書くと報告している。`aidlc-state.md` は workflow を 1 度走らせるまで生成されないので、初回の commit には載らない。それを含む最初の commit の前に中身を確認する。

## 同梱から外したもの

| 対象 | 理由 |
| ---- | ---- |
| `settings.json` の env 6 変数 (`CLAUDE_CODE_USE_BEDROCK`, `AWS_REGION`, `ANTHROPIC_DEFAULT_{FABLE,OPUS,SONNET,HAIKU}_MODEL`) | AWS Bedrock 固定を外し、現在の Claude Code のモデル設定のまま動かす。AWS 認証情報と Bedrock のモデルアクセス申請が不要になる。`AWS_AIDLC_DEFAULT_SCOPE=classic` は AI-DLC 側の設定なので残した |
| `settings.json` の `"model": "opus[1m]"` | 同上。session のモデルを AI-DLC が上書きしないようにする |
| `dist/claude/.mcp.json` | context7 (要 `CONTEXT7_API_KEY`) と AWS 系 4 本 (aws-mcp / aws-pricing / aws-iac / aws-serverless、要 uvx)。scout は Rust CLI で AWS へのデプロイ対象を持たない |

削除前の原本は `.claude/settings.json.aidlc-orig` に残してある。

## 同梱のまま有効にした挙動

`settings.json` が session 全体へ効かせるもの。AI-DLC を使わない作業中も効く。

| 項目 | 内容 |
| ---- | ---- |
| `permissions.allow` | `Read` `Edit` `Write` `Bash` `Glob` `Grep` `Task` `WebSearch` を素で並べる。`Bash` は無条件許可で、scout がこれまで `settings.local.json` に持っていた 4 本の限定 allow より広い |
| `effortLevel` | `xhigh` |
| `statusLine` | `aidlc-statusline.ts` へ差し替え |
| `companyAnnouncements` | session 開始ごとに 33 stage の表を出す |
| hook | PreToolUse ほか 18 本を登録する |

`.claude/CLAUDE.md` (18.7 KB) と `.claude/skills/aidlc/SKILL.md` (50.8 KB) が context に載る。

## workflow を張っていないときに guard が素通りするか

`aidlc-state-transition-guard` `aidlc-reviewer-scope` `aidlc-review-freeze` は PreToolUse の matcher が `Read|Edit|Write|Bash|Glob|Grep` で、`aidlc-plan-approval-guard` は `Edit|Write|Bash|Task` である。AI-DLC を起動していない普通の Rust 作業も全部この matcher に当たる。

評価用 worktree で、workflow 未起動の状態の PreToolUse payload を 6 本の hook へ流した。`Edit src/main.rs` `Write src/new.rs` `Bash cargo check` `Read src/main.rs` `Task` の 5 ケースすべてで exit 0・出力なし、つまり素通りだった。hook が実際に走ったことは doctor の `Hooks last fired: review-freeze ..., reviewer-scope ..., plan-approval-guard ...` の timestamp で確かめた。

## bun は mise shim を経由させず実体の絶対パスで呼ぶ

`bun` は `~/.local/share/mise/shims/bun` (mise 本体への hard link) に解決される。この shim 経由で `.claude/tools/aidlc-utility.ts` を実行すると、AI-DLC の出力ではなく package manager の選択 picker (`? Choose the agent >`) が stdout に出て exit 0 で終わることがある。成功した実行を 3 回挟んだ後にも再発したので、初回解決だけの現象ではない。実体 `~/.local/share/mise/installs/bun/latest/bin/bun` では再現しなかった。

空振りは exit 0 で標準エラーも空なので、hook が黙って何もしない状態と区別が付かない。`aidlc-plan-approval-guard` や `aidlc-state-transition-guard` が空振りすると、承認ゲートの無い AI-DLC が動いているように見える。

対策として `settings.json` の hook command 18 本と statusLine を実体 bun の絶対パスへ書き換えた。`permissions.allow` は絶対パス形と `bun` 形の両方を許可する。

hook が発火しているかは doctor の `Hooks last fired:` 行で見る。`Hook heartbeats: not yet fired` は「まだ発火していない」と「発火して空振りした」を区別しないので、この判定には使えない。

bun を上げると `latest` の実体が入れ替わる。その場合は「そのファイルが無い」と鳴って落ちるので、空振りより検出できる。

## Bash コマンドを改行で分けると runtime-graph の再構築 hook が空振りする

**`bun` という語とツールのパスは同じ行に置く。** 分けると hook が黙って起動せず、学びの儀式が動かなくなる。

`.claude/hooks/aidlc-rebuild-stage-graph.ts` は PostToolUse で Bash コマンドの文字列を読み、`aidlc-runtime.ts compile` を起動する。それが intent 記録直下の `runtime-graph.json` を書く。判定は `aidlc-lib.ts` の `classifyRuntimeCompileCommand` が持ち、`runtimeCompileReport` は `\bbun\b.*<harness>/tools/aidlc-orchestrate\.ts\b.*\breport\b` の 1 本である。`s` フラグが無いので `.*` は改行を越えない。

2026-08-29 の Reverse Engineering 実行で、承認コマンドを 1 行目 `BUN=$(mise which bun); cd <repo>`、2 行目 `"$BUN" .claude/tools/aidlc-orchestrate.ts report --result approved` の 2 行で出した。分類器は `pass` を返し、hook は起動せず、`runtime-graph.json` は作られなかった。`aidlc-learnings.ts surface` はそのファイルを要求するので失敗し、**学びの候補抽出と `persist` が走らないまま stage が承認された**。承認後は `surface` が `slug mismatch` を返すので、その stage の儀式はもう実行できない。

`"$BUN"` の使用は原因ではない。`mise which bun` の中に `bun` の語が入るため `\bbun\b` は当たる。効いているのは改行だけである。

| コマンドの形 | `classifyRuntimeCompileCommand` |
| ------------ | ------------------------------- |
| 1 行、`"$BUN"` を使用 | fire |
| 改行を挟む、`"$BUN"` を使用 | pass |
| 改行あり、`bun` とパスが同じ行 | fire |
| 1 行、ドキュメントどおりの形 | fire |

空振りは exit 0 で出力も無く、`.aidlc-hooks-health` の drop にも残らない。唯一の手がかりは `/aidlc --doctor` の `! [runtime-graph-missing]` 1 行で、これは advisory なので 51 passed / 0 failed のまま出る。

失われた儀式の代わりに、その stage で起きたことは `<record>/inception/reverse-engineering/memory.md` へ手で 5 件書き足した。観察ノートは stage 記録の一部として残るが、`project.md` の規則にはならない。

## 検証

```
bun .claude/tools/aidlc-utility.ts doctor
```

2026-08-28 の結果は 50 passed / 0 failed。

## 使い方

session 内で `/aidlc --doctor` -> `/aidlc --status` -> `/aidlc <やりたいこと>`。

初回は `/aidlc --scope express <やりたいこと>` を勧める。express は RE・RA・CG・BT の 4 stage で engine を端から端まで通す。`AWS_AIDLC_DEFAULT_SCOPE=classic` のままだと INCEPTION 9 + CONSTRUCTION 7 + OPERATION 7 の 23 stage が `effortLevel: xhigh` で走り、reverse engineering が scout の 29,000 行超の Rust に当たる。classic は出力の質を評価する段になってから使う。

## 撤去

`.claude/` 側は AI-DLC が持ち込んだ 12 個の名前だけを消す。scout 既存の `OUTCOME.md` `rules/CONVENTIONS.md` `rules/CORRECTIONS.md` `agent-memory/` `settings.local.json` `tools.json` `workspace/` `worktrees/` `dead_code_analysis.md` は残す。

```
cd /Users/thkt/GitHub/cli/scout/.claude
mv agents aidlc-common hooks knowledge scopes sensors skills tools \
   CLAUDE.md settings.json settings.json.aidlc-orig settings.local.json.example rules/aidlc.md ~/.Trash/
cd .. && mv aidlc ~/.Trash/
```

`.gitignore` の `# AI-DLC` 節も外す。
