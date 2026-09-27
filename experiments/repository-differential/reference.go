// Executable contract models, not a port of the scout CLI.
// Models use documented contracts, fixtures and standard-library primitives.
package main

import (
	"bufio"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/netip"
	"os"
	"regexp"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

type Input struct {
	Op string `json:"op"`
	A  string `json:"a"`
	B  string `json:"b"`
	N  uint64 `json:"n"`
}

func fold(s string) string { return strings.NewReplacer("\r", " ", "\n", " ").Replace(s) }
func inline(s string) string {
	return strings.NewReplacer("|", `\|`, "[", `\[`, "]", `\]`, "(", `\(`, ")", `\)`).Replace(fold(s))
}
func cut(s string, n uint64) string {
	if uint64(len(s)) <= n {
		return s
	}
	p := int(n)
	for p > 0 && !utf8.RuneStart(s[p]) {
		p--
	}
	return s[:p]
}
func u64(s string) (uint64, bool) {
	s = strings.TrimPrefix(s, "+")
	if s == "" {
		return 0, false
	}
	for _, c := range s {
		if c < '0' || c > '9' {
			return 0, false
		}
	}
	n, e := strconv.ParseUint(s, 10, 64)
	return n, e == nil
}
func validName(s string) bool {
	if s == "" || s == "." || s == ".." {
		return false
	}
	for _, c := range s {
		if !strings.ContainsRune("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.", c) {
			return false
		}
	}
	return true
}
func ref(s string) bool {
	if s == "" || s == "@" || strings.Contains(s, "..") || strings.Contains(s, "@{") || strings.HasSuffix(s, ".") {
		return false
	}
	for _, c := range s {
		if c < 33 || c == 127 || strings.ContainsRune("~^:?*[\\", c) {
			return false
		}
	}
	for _, p := range strings.Split(s, "/") {
		if p == "" || strings.HasPrefix(p, ".") || strings.HasSuffix(p, ".lock") {
			return false
		}
	}
	return true
}
func private(s string) bool {
	a, e := netip.ParseAddr(s)
	if e != nil {
		panic(e)
	}
	if a.Is6() {
		b := a.As16()
		zero := true
		for _, v := range b[:12] {
			zero = zero && v == 0
		}
		if a.Is4In6() {
			a = a.Unmap()
		} else if zero {
			a = netip.AddrFrom4([4]byte{b[12], b[13], b[14], b[15]})
		}
	}
	for _, prefix := range []string{"0.0.0.0/8", "10.0.0.0/8", "127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10", "255.255.255.255/32", "::/128", "::1/128", "fe80::/10", "fc00::/7"} {
		if netip.MustParsePrefix(prefix).Contains(a) {
			return true
		}
	}
	return false
}

var mentionRE = regexp.MustCompile(`<@([^>]*)>`)

func model(v Input) any {
	s := v.A
	switch v.Op {
	case "inline":
		return inline(s)
	case "heading":
		return fold(s)
	case "truncate":
		if uint64(len(s)) <= v.N {
			return s
		}
		prefix := cut(s, v.N)
		if i := strings.LastIndexByte(prefix, '\n'); i >= 0 {
			prefix = prefix[:i+1]
		}
		return fmt.Sprintf("%s\n\n(truncated: showing %d / %d bytes)", prefix, len(prefix), len(s))
	case "fence":
		max, run := 2, 0
		for _, c := range s {
			if c == '`' {
				run++
				if run > max {
					max = run
				}
			} else {
				run = 0
			}
		}
		return strings.Repeat("`", max+1)
	case "yaml":
		return strings.NewReplacer("\\", `\\`, "\"", `\"`, "\n", `\n`, "\r", `\r`, "\t", `\t`, "\x00", "").Replace(s)
	case "yaml_field":
		value := cut(s, 450)
		if value != s {
			value += "…"
		}
		return "title: \"" + model(Input{Op: "yaml", A: value}).(string) + "\"\n"
	case "markers":
		lines := strings.Split(s, "\n")
		for i, l := range lines {
			if len(l) >= 3 && (l[:3] == "---" || l[:3] == "...") && (len(l) == 3 || strings.ContainsRune(" \t\r", rune(l[3]))) {
				rest := l[3:]
				if strings.Trim(rest, " \t\r") == "" {
					rest = ""
				}
				lines[i] = "***" + rest
			}
		}
		return strings.Join(lines, "\n")
	case "encode_path":
		var b strings.Builder
		for _, c := range []byte(s) {
			if c < 32 || c >= 127 || strings.ContainsRune(" ?#%&+@[];=", rune(c)) {
				fmt.Fprintf(&b, "%%%02X", c)
			} else {
				b.WriteByte(c)
			}
		}
		return b.String()
	case "repo":
		s = strings.TrimPrefix(strings.TrimPrefix(s, "https://github.com/"), "http://github.com/")
		s = strings.TrimRight(s, "/")
		s = strings.TrimSuffix(s, ".git")
		p := strings.Split(s, "/")
		if len(p) < 2 || !validName(p[0]) || !validName(p[1]) {
			return nil
		}
		return []string{p[0], p[1]}
	case "ref":
		return ref(s)
	case "path":
		return s != "" && !strings.HasPrefix(s, "/") && !strings.ContainsAny(s, "\x00\n\r") && !strings.Contains("/"+s+"/", "/../")
	case "range":
		s = strings.TrimSpace(s)
		a, b, has := strings.Cut(s, "-")
		start, ok := u64(strings.TrimSpace(a))
		if !ok || start == 0 {
			return nil
		}
		if !has {
			return []any{uint64(1), start}
		}
		if strings.TrimSpace(b) == "" {
			return []any{start, nil}
		}
		end, ok := u64(strings.TrimSpace(b))
		if !ok || end < start {
			return nil
		}
		return []any{start, end}
	case "base64":
		clean := strings.Map(func(c rune) rune {
			if unicode.IsSpace(c) {
				return -1
			}
			return c
		}, s)
		out, e := base64.StdEncoding.Strict().DecodeString(clean)
		if e != nil {
			return nil
		}
		return hex.EncodeToString(out)
	case "size":
		if v.N < 1024 {
			return fmt.Sprintf("%d B", v.N)
		}
		if v.N < 1048576 {
			return fmt.Sprintf("%.1f KB", float64(v.N)/1024)
		}
		return fmt.Sprintf("%.1f MB", float64(v.N)/1048576)
	case "content_type":
		s = strings.ToLower(strings.TrimSpace(strings.SplitN(s, ";", 2)[0]))
		return s == "" || strings.HasPrefix(s, "text/") || s == "application/xml" || (strings.HasPrefix(s, "application/") && strings.HasSuffix(s, "+xml"))
	case "ip":
		return private(s)
	case "retry":
		if n, ok := u64(s); ok {
			return n
		}
		t, e := http.ParseTime(s)
		if e != nil || t.Unix() < 0 {
			return nil
		}
		if uint64(t.Unix()) < v.N {
			return uint64(0)
		}
		return uint64(t.Unix()) - v.N
	case "retry_cap":
		return v.N <= 300
	case "status":
		code, exit, retry := "UNKNOWN", 104, false
		switch {
		case v.N == 408 || v.N == 429 || (v.N >= 500 && v.N <= 599):
			code, exit, retry = "TEMP_FAILURE", 75, true
		case v.N == 404:
			code, exit = "NOT_FOUND", 66
		case v.N == 401 || v.N == 403:
			code, exit = "USAGE_ERROR", 64
		case v.N >= 400 && v.N <= 499:
			code, exit = "DATA_ERROR", 65
		}
		return []any{code, exit, retry}
	case "config":
		if strings.TrimSpace(s) == "" {
			return nil
		}
		n, ok := u64(strings.TrimSpace(s))
		if !ok {
			return nil
		}
		if v.B == "SCOUT_MAX_RETRIES" {
			if n > 10 {
				return nil
			}
		} else if n < 1 || n > 600 {
			return nil
		}
		return n
	case "mention":
		return mentionRE.ReplaceAllStringFunc(s, func(raw string) string {
			id, label, _ := strings.Cut(raw[2:len(raw)-1], "|")
			if id == "" || strings.Contains(id, "<") || strings.ContainsFunc(id, unicode.IsSpace) {
				return raw
			}
			name := map[string]string{"U123": "Bob", "U100": "Alice"}[id]
			if name == "" {
				name = label
			}
			if name == "" {
				name = id
			}
			return "@" + name
		})
	default:
		panic("unknown op: " + v.Op)
	}
}
func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 4096), 4*1024*1024)
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	for scanner.Scan() {
		var v Input
		if e := json.Unmarshal(scanner.Bytes(), &v); e != nil {
			panic(e)
		}
		b, e := json.Marshal(model(v))
		if e != nil {
			panic(e)
		}
		fmt.Fprintln(out, string(b))
	}
	if e := scanner.Err(); e != nil {
		panic(e)
	}
}
