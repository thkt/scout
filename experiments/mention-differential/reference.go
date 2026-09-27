// Independent reference written from mention_tests.rs before reading mention.rs.
package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"os"
	"regexp"
	"strings"
	"unicode"
)

var token = regexp.MustCompile(`<@([^>]*)>`)

func substitute(text string) string {
	cache := map[string]string{"U123": "Bob", "U100": "Alice", "EMPTY": ""}
	return token.ReplaceAllStringFunc(text, func(raw string) string {
		body := raw[2 : len(raw)-1]
		id, label, _ := strings.Cut(body, "|")
		if id == "" || strings.Contains(id, "<") || strings.ContainsFunc(id, unicode.IsSpace) {
			return raw
		}
		name := cache[id]
		if name == "" {
			name = label
		}
		if name == "" {
			name = id
		}
		return "@" + name
	})
}

func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 4096), 4*1024*1024)
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	for scanner.Scan() {
		input, err := hex.DecodeString(scanner.Text())
		if err != nil {
			panic(err)
		}
		fmt.Fprintln(out, hex.EncodeToString([]byte(substitute(string(input)))))
	}
	if err := scanner.Err(); err != nil {
		panic(err)
	}
}
