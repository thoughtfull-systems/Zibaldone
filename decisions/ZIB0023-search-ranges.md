<!-- The decision identifier precedes the sentence-case title. -->

<!-- vale Google.Headings = NO -->

# ZIB0023 Define search ranges

<!-- vale Google.Headings = YES -->

## Context

Search conditions support typed comparisons, and saved searches retain their expressions. A range
currently needs two conditions. The owner requests `..` for a range that excludes its end and `...`
for a range that includes its end.

## Decision

Recognize an unquoted range after a field's `=` operator. Both forms include the start. Two periods
exclude the end, and three periods include it. Both bounds use the field's existing ordered
comparison rules for dates, decimals, integers, times, or timestamps.

Keep both bounds in one predicate so a list field needs one item within the range. Two independent
comparisons could match different items, leaving no item inside the requested range.

Validate both bounds before running the query. Reject a start greater than the end. Equal bounds
form an empty range with `..` and match that value with `...`. Quoting a whole value keeps periods
literal, so exact text searches can contain either sequence.

The [search ranges specification][ranges] defines the syntax and behavior.

[ranges]: ../specs/search-ranges.md
