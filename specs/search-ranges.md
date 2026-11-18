# Search ranges

This specification defines range conditions in desktop search expressions, following
[decision ZIB0023][decision]. A [saved search][saved] retains the expression as text.

## Syntax

A range follows a field's equality operator and has two bounds separated by `..` or `...`. Supply
both bounds and write the condition without spaces inside the value.

```text
@published-on=2026-01-01..2027-01-01
@published-on=2026-01-01...2026-12-31
@amount=-1.5...2
```

Both forms include the start. Two periods exclude the end, and three periods include the end. A
quoted value such as `@url="a..b"` treats its contents as one literal value.

## Comparisons

Ranges support date, decimal, integer, time, and timestamp fields with value comparisons. Each bound
follows the field's existing value rules. Decimal comparisons preserve numeric precision, and times
with omitted seconds compare as if their seconds were zero. Timestamps use integer milliseconds
since the Unix epoch.

A note matches when its field contains a value within the range. For a list field, one item must
satisfy both bounds. A missing field, an empty list, or an invalid stored value doesn't match.
Negation reverses that result, so `NOT` applied to a range includes missing fields.

The query reports an error for malformed bounds, an invalid value, a field without ordered
comparisons, or a start greater than the end. It validates ranges under `OR` and `NOT` as well.
Equal bounds form an empty range with `..` and match that value with `...`.

## Timestamp inputs

Timestamp comparisons and range bounds accept calendar inputs alongside integer milliseconds,
following [decision ZIB0024][timestamps]. Each input supplies a complete `YYYY-MM-DD` date,
optionally followed by `THH`, `THH:MM`, or `THH:MM:SS`. Seconds can have a decimal fraction of one
to three digits. Components use two digits, and hours use a 24-hour clock. Fractions denote parts of
a second: `.1` means 100 milliseconds.

An input without a timezone uses the machine's local timezone for the specified date. A suffix of
`Z` uses Coordinated Universal Time (UTC). A suffix of `+HH:MM` or `-HH:MM` supplies an explicit
offset. These suffixes apply to date-only inputs and every supported time precision. The query
rejects invalid dates, times, and offsets. A local time that occurs twice or doesn't exist during a
clock change requires an explicit offset.

A calendar input denotes the period at its supplied precision. A date covers one local day, an hour
covers that hour, a minute covers that minute, and whole seconds cover that second. One fractional
digit covers 100 milliseconds, two cover 10 milliseconds, and three identify one millisecond.
Integer inputs identify one exact millisecond.

The period starts with omitted components set to zero and ends with them set to their greatest valid
values. Resolve each boundary with the offset for its date in the chosen timezone. Local days can
have 23 or 25 hours when clocks change. Use calendar boundaries rather than adding 24 hours to the
first instant.

### Calendar years and months

Calendar inputs also accept a four-digit year as `YYYY` and a year and month as `YYYY-MM`. A year
covers January 1 at midnight through December 31 at `23:59:59.999`. A month covers its first day at
midnight through its final day at `23:59:59.999`. Month lengths follow the calendar, including leap
years. Omitted months and days begin at one. A time component requires a complete date.

These inputs accept the same local timezone default, explicit timezone suffixes, comparison
operators, and range endpoint rules as complete dates. For example:

```text
@captured-at=2026
@captured-at=2026-11
@captured-at=2026-11...2026-12
@captured-at=2026-11..2026-12
```

The third query includes November and December. The fourth includes November and excludes December.
A plain value of exactly four unsigned digits denotes a calendar year. Other plain integers keep
their millisecond interpretation. The `ms:` prefix always denotes exact epoch milliseconds, so
`@captured-at=ms:2026` searches for the instant 2026 milliseconds after the Unix epoch. This prefix
also works in range bounds.

### Timestamp conditions

Expand timestamp periods before comparing values or checking bound order. Equality with a single
timestamp matches its whole period. These queries match the same captures:

```text
@captured-at=2026-02-19
@captured-at=2026-02-19...2026-02-19
```

A range starts at the first millisecond of its starting period. An inclusive `...` end includes the
last millisecond of its ending period. An exclusive `..` end excludes the ending period, stopping at
its first millisecond. In New York, this input:

```text
@captured-at=2026-02-19...2026-03-15
```

has these explicit boundaries, with the offset change that occurs between the two dates:

```text
@captured-at=2026-02-19T00:00:00.000-05:00...2026-03-15T23:59:59.999-04:00
```

Scalar `<` and `>=` compare with the first instant of the supplied period. Scalar `<=` and `>`
compare with its last instant. Thus `<=2026-02-19` includes that whole day, and `>2026-02-19` starts
after that day. Values with three fractional digits and integer timestamps keep exact comparisons.

A saved search without offsets uses the local timezone each time it runs. Explicit offsets keep its
instants the same across machines.

## Platforms

The desktop search box and saved-search definitions use this syntax. The command-line search retains
its existing word search.

[decision]: ../decisions/ZIB0023-search-ranges.md
[saved]: saved-searches.md
[timestamps]: ../decisions/ZIB0024-search-timestamps.md
