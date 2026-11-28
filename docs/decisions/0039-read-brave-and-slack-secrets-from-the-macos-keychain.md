---
status: "proposed"
date: 2026-11-28
---

# Brave と Slack の秘密を、環境変数に無いとき macOS の Keychain から読む

## Context and Problem Statement

[Issue #562](https://github.com/thkt/scout/issues/562) は、`BRAVE_SEARCH_API_KEY` と `SLACK_TOKEN` が環境変数に無いとき、macOS では Keychain から読むことを求めている。基準版は `6bf7b7c`。

2026-11-28、GUI アプリから起動した Claude Code が `scout search` を実行し、`BRAVE_SEARCH_API_KEY not set` で失敗した。利用者は `~/.zshrc` からこの変数を export していたが、GUI アプリの子プロセスとして動くエージェントの shell には、その export が 1 つも届いていなかった。同じ shell から `security find-generic-password` で Keychain を読むことはできた。

それまで `BraveClient` と `SlackClient` は、`Redacted::from_env_var` で環境変数だけを読んでいた。GitHub の token だけは、`src/token_source.rs` が環境変数の次に `gh auth token` を実行して読む（[DR-0018](0018-github-token-resolution-precedence.md)）。

## Decision Drivers

- shell の起動ファイルを読まないプロセスから動くエージェントも、`search`、`research`、Slack permalink の `fetch` を使えるようにする。
- 秘密を環境変数より広い範囲に置かない。`launchctl setenv` のように、GUI のアプリ全部に秘密を渡す方法は採らない。
- 秘密を使わないコマンドは、秘密を読むための subprocess を起動しない。
- テストの結果を、手元の Keychain の中身に左右させない。
- 秘密は `Redacted` で運び、`security` の stderr を log に出さない（[DR-0015](0015-redacted-mandatory-secret-carrier.md)）。

## Considered Options

- 環境変数に無いとき、macOS では環境変数と同じ名前を service 名に持つ Keychain の項目を読む
- 利用者が GUI アプリを shell から起動し、環境変数を引き継がせる
- 利用者が `launchctl setenv` でログイン時に環境変数を設定する
- scout 専用の設定ファイルに秘密を書く

## Decision Outcome

Chosen option: 「環境変数に無いとき、macOS では環境変数と同じ名前を service 名に持つ Keychain の項目を読む」。秘密は Keychain から出ず、どこから起動した scout でも同じ項目を読める。

| 項目 | 決定 |
| --- | --- |
| 対象の秘密 | 次の 2 つだけ: `BRAVE_SEARCH_API_KEY`、`SLACK_TOKEN`。GitHub の token は `gh auth token` で読めるので対象にしない |
| 項目の名前 | service 名を環境変数と同じ名前にし、account は指定しない。利用者ごとに名前を設定させずに済む |
| 読む順序 | 環境変数に値があればそれを使い、Keychain は読まない。環境変数が無いか空白だけのときに Keychain を読む |
| 読む OS | macOS だけ。それ以外の OS では今までどおり `ApiKeyNotSet` と `TokenNotSet` を返す |
| 読み方 | `/usr/bin/security find-generic-password -s <変数名> -w` を 5 秒の timeout 付きで実行し、stdout を `Redacted::new` で包む。絶対パスで呼ぶのは、PATH の前の方にある別の `security` に秘密を決めさせないためである。stderr は捨て、log には終了コードだけを書く。項目が無いとき（終了 44）は `debug!`、それ以外の失敗は `warn!` で書く。終了コードが 0 でないとき、timeout したとき、起動できないときは、項目が無いときと同じエラーを返す |
| Slack の値の検査 | Keychain から読んだ token にも `xoxp-` の検査を掛ける（[DR-0022](0022-slack-user-token-prefix-enforced-at-construction.md)） |
| 読む時点 | 秘密を使うコマンドが初めて client を使うときだけ。Brave の client も、Slack と GitHub と同じく `OnceCell` で初めて使うときに作る。`repo-*` と Slack 以外の `fetch` は `security` を実行しない |
| 実装の置き場所 | `src/secret_source.rs` の `SecretSource` trait と `resolve`。`BraveClient::from_secret` と `SlackClient::from_secret` は、解決済みの `Option<Redacted>` を受け取る。`Redacted::from_env_var` は使う所が無くなったので削除する |
| Keychain を読まない指定 | `SCOUT_NO_KEYCHAIN=1` で Keychain を読まない。キーが無いときの動作を確かめる結合テストは、これを設定する |

`ScoutBuilder::for_test` は `StaticSecretSource(None)` を使い、ユニットテストは実際の環境変数も Keychain も読まない。`resolve` は `security` を実行する関数を引数で受け取るので、テストは成功、終了 44、0 以外の終了、timeout、起動の失敗を、実際の Keychain なしで再現できる。

### Consequences

- Good, because GUI アプリから起動したエージェントも、Keychain に登録した秘密で `search`、`research`、Slack permalink の `fetch` を使える。
- Good, because Brave の client を初めて使うときに作るので、`repo-*` と Slack 以外の `fetch` は Brave の key を読まない。
- Bad, because Keychain の項目のアクセス制御が `security` を許していないと、macOS が許可のダイアログを出す。応答が無ければ 5 秒待ってから、キーが無いときと同じエラーになる。ダイアログが出る状況は試していない。
- Bad, because 秘密が見つからないとき、macOS では毎回 `security` を 1 回実行する。
- Bad, because Keychain がロックされているときなど、項目はあるが読めないときも、項目が無いときと同じ「not set」のエラーになる。原因は `warn!` の終了コードにだけ出る。
- Neutral, because 環境変数に値があれば、Keychain の項目より環境変数が優先される。Keychain の値に切り替えるには、環境変数を外す必要がある。

### Confirmation

`src/secret_source.rs` の T-KC001 から T-KC009 で、環境変数の優先、Keychain への切り替え、空白だけの環境変数、終了 44 と 0 以外の終了、起動の失敗、timeout、空の値、`SCOUT_NO_KEYCHAIN=1`、macOS 以外では Keychain を読まないことを確かめる。`src/tools/builder_tests.rs` の T-SB008 から T-SB011 で、Brave と Slack の client が注入した source から作られること、Brave の key が 1 回だけ読まれること、`repo-overview` と Slack 以外の `fetch` が秘密を読まないことを確かめる。`tests/cli_integration.rs` の T-C003、T-C008、T-C038 は `SCOUT_NO_KEYCHAIN=1` を設定して、キーが無いときの終了 64 を確かめる。環境変数を消して scout を起動する `tests/common/mod.rs` の `scout_with_env` も `SCOUT_NO_KEYCHAIN=1` を設定する。設定しないと、Keychain に `SLACK_TOKEN` の項目がある Mac では、T-HP009 が本物の token で Slack API を呼び、終了 64 ではなく 66 になる。

## More Information

GUI アプリを shell から起動する案は採らない。Dock や Spotlight から起動すると元に戻り、起動の仕方を毎回守る必要がある。

`launchctl setenv` の案は採らない。ログインしている間、GUI のアプリ全部に秘密が渡る。

scout 専用の設定ファイルの案は採らない。秘密を平文のファイルに置くことになり、Keychain より守りが弱い。

### Reassessment Triggers

- Linux や Windows で、同じ理由で秘密が届かない事例が出た場合（Secret Service や Credential Manager を検討する）
- Keychain の許可のダイアログで `security` が止まる事例が出た場合
- `GITHUB_TOKEN` も `gh auth token` で読めない環境から使う事例が出た場合
