# todofmt

> [!WARNING]
> **Vibe coded.** Dieses Tool wurde in einer Session von einer KI geschrieben,
> ohne dass ein Mensch den Code Zeile für Zeile gelesen hätte. Es tut, was die
> Tests abdecken — für alles darüber hinaus ohne Garantie. Wer es benutzt:
> kurz selbst reinschauen.

Multi-Key-Sorter **und Formatter** für
[todo.txt](https://github.com/todotxt/todo.txt)-Dateien, geschrieben in Rust
(blazingly fast ⚡, ~ms für 70k Zeilen). Konzipiert als Formatter-Backend für
[Helix](https://helix-editor.com) (`todotxt`-Language), aber auch als
eigenständiges CLI-Tool brauchbar.

## Warum

Helix bringt für `todotxt` einen Formatter mit: plain `/usr/bin/sort` mit
`auto-format = true`. Das sortiert jede Zeile alphabetisch — erledigte
Tasks (`x …`) landen damit zwangsläufig *irgendwo im Alphabet*, Daten
verhalten sich zufällig je nachdem, ob sie am Zeilenanfang stehen.

todofmt kennt die todo.txt-Semantik und sortiert nach echten Keys —
mehrere davon, positionsbasiert wie SQL `ORDER BY`: Die Reihenfolge der
`--sort`-Flags bestimmt die Priorität.

```
todofmt --sort completed --sort timestamp   ≠   todofmt --sort timestamp --sort completed
```

Außerdem wird jede Zeile normalisiert (abschaltbar, siehe `--no-format-lines`):

```
Eingabe:  x @web (B) 2026-10-07 Kräuter-tee +essen due:2026-10-09
Ausgabe:  x (B) 2026-10-07 Kräuter-tee +essen @web due:2026-10-09
```

## Build & Install

```sh
cargo install --path .
```

## Usage

```sh
todofmt [OPTIONEN] [DATEI]        # ohne DATEI: stdin → stdout (Filter-Style)
```

### Optionen

| Flag | Wirkung |
|---|---|
| `-s, --sort KEY[:DIR]` | Sortierkey(s), wiederholbar und/oder kommasepariert. Reihenfolge = Priorität. `DIR` = `asc` (Default) oder `desc`. Überschreibt die Default-Kette komplett |
| `--no-sort` | Keine Zeilenreihenfolge; nur Normalisierung (mit `--no-format-lines`: gar nichts) |
| `--no-format-lines` | Nur Zeilenreihenfolge ändern; Zeileninhalt byte-genau lassen (auch kein Prio-Stripping) |
| `-r, --reverse` | Dreht das Endergebnis um |
| `-c, --check` | Prüft nur: Exit 0 wenn bereits in Zielform, Exit 1 sonst (silent bei Erfolg). Für Pre-Commit-Hooks/CI; unvereinbar mit `--in-place` |
| `-i, --in-place` / `-w, --write` | DATEI zurückschreiben statt stdout |

Ohne `--sort` gilt: `completed,text` — offene Tasks alphabetisch oben,
erledigte versickern unten.

### Sort-Keys

| Key | Aliases | Sortiert nach |
|---|---|---|
| `completed` | `done`, `checked` | Erledigt-Status (`asc`: offen zuerst, erledigt nach unten) |
| `timestamp` | `timestamps`, `date`, `time` | Erledigungsdatum bei `x`-Zeilen, sonst Erstellungsdatum; undatierte nach hinten (`NULLS LAST`) |
| `text` | `alphabetical`, `alphabetically`, `alpha`, `name` | Task-Text (Unicode-Collation via `feruca` — `Ärzte` sortiert bei `apfel`, nicht hinter `Zebra`) |
| `priority` | `prio` | `(A)`–`(Z)`, ohne Prio nach hinten |
| `due` | — | `due:YYYY-MM-DD` im Text, ohne nach hinten |
| `project` | `projects`, `tag`, `tags` | Erstes `+projekt`-Tag (Gruppierung), ohne Tag nach hinten |

Ohne `--sort` gilt: `completed,priority,project,timestamp:desc` — erledigte Tasks
unten, dann Prio-Tiers `(A)`→`(Z)`→ohne, darin Gruppen nach `+tag`, am Ende
nach Datum (Neuestes zuerst, Undatiertes davor; bei Gleichstand zählt die
Eingabe-Reihenfolge). Alphabetisch sortiert wird nur auf expliziten Wunsch
(`-s text`) — innerhalb einer Textzeile bleibt alles unangetastet.

Die Default-Kette lässt sich 1:1 als explizites Komma-Argument schreiben:

```sh
todofmt -s completed,priority,project,timestamp:desc todo.txt
```

### Normalisierung (Default an, `--no-format-lines` schaltet ab)

Kanonische Form pro Zeile (todo.sh-De-facto-Order, erledigte Tasks ohne Prio):

```
x  <done-date>  <created-date>  Text…  +projekte  @kontexte  key:value
     (A) (prio) ── nur bei offenen Tasks
```

- Struktur-Präfix neu ordnen; erledigte Tasks verlieren ihre Priorität
  (Spec-Hygiene — hält den aktiven Prio-Raum sauber; Sortieren sieht die
  Original-Prio, gestrippt wird erst beim Schreiben)
- Tags (`+projekt`, `@kontext`, bekannte `key:value`
  wie `due:`/`t:`/`rec:`/`h:`/`pri:`/`thresh:`) ans Zeilenende — jeweils alphabetisch
- **Der Task-Text selbst bleibt byte-genau stehen** — nur Struktur-Tokens und
  Tags wandern
- Konservativ: nur der *führende* Struktur-Block wird erkannt. Ein `(A)` mitten
  im Text bleibt Text (könnte „Option (A) wählen" sein), URLs und Uhrzeiten
  (`12:30`) werden nicht als Tags verschoben

Bewusst weggelassen: `--keep-empty`/`--trim` (Leerzeilen tragen keine
Ordnungs-Semantik, Whitespace bleibt verbatim), Case-Flags (die
Root-Collation behandelt Groß-/Kleinschreibung richtig) und granulare
Tags-Flags (`--sort-tags` o.ä.) — Formatter haben *eine* kanonische Meinung,
keine Kompositions-Flags.

### Subtasks (Markor/Simpletask-Konvention)

Eingerückte Zeilen gehören zum darüberliegenden Elternteil (so rendert es
Markor). todofmt behandelt sie als **Blöcke**:

- Sortiert wird Block-weise nach den Keys des Elternteils — Kinder wandern
  nie von ihrem Parent weg, egal welche Keys greifen
- Kinder behalten ihre Erfassungs-Reihenfolge innerhalb des Blocks
- Normalisierung gilt auch für Kinder (kanonische Form, Tags, Prio-Strip
  bei `x`-Zeilen) — die **Einrückung bleibt byte-genau** erhalten
- `--reverse` dreht Block-Reihenfolge, nicht Block-Inhalt

Kein Flag nötig: flache Dateien verhalten sich exakt wie vorher, eingerückte
Zeilen gibt es nur, wenn sie bewusst Struktur sind.

### Beispiele

```sh
# Default: offen A–Z oben, erledigt unten, Zeilen normalisiert (Helix-Verhalten)
todofmt todo.txt

# Erledigte oben, neueste zuerst — danach offene nach Datum
todofmt --sort completed:desc --sort timestamp todo.txt

# Prio first, Text als Tiebreaker, Ergebnis gespiegelt
todofmt -s prio,text -r todo.txt

# Nur sortieren, Zeilen nicht anfassen
todofmt --no-format-lines todo.txt

# Erledigte Prios fliegen automatisch raus (Teil der Normalisierung) + zurückschreiben
todofmt -w todo.txt

# Pre-Commit-Hook: ist die Datei in Zielform?
todofmt -c todo.txt || echo "erst sortieren!"

# Filter-Style: stdin → stdout
cat todo.txt | todofmt -s completed,text > sorted.txt
```

## Helix-Integration

In `~/.config/helix/languages.toml` (ersetzt das eingebaute plain `sort`):

```toml
[[language]]
name = "todotxt"
formatter = { command = "todofmt" }
```

Da die eingebaute `todotxt`-Sprache `auto-format = true` setzt, wird bei
jedem Speichern sortiert **und normalisiert**: offene Punkte A–Z oben,
abgehakte rutschen nach unten, Tags stehen ordentlich am Zeilenende.
Andere Ordnung? `args = ["--sort", "completed:desc", "--sort", "timestamp"]`.
Nur sortieren ohne Zeilen-Pflege: `args = ["--no-format-lines"]`.
