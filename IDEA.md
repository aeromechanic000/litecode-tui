# Design Idea of Global Instruction

Consider the best_skill.md introduced from https://github.com/microsoft/SkillOpt. Here are some final best skills

# DocVQA Skill

## Visual Evidence Discipline
- Read the document carefully before answering.
- Prefer the smallest exact text span that answers the question.

- For questions asking for a value, count, page number, date, or graph reading, return only the requested value span; omit nearby labels, category names, units, or explanatory words unless the question explicitly asks for them.
- When several nearby strings look similar, choose the one whose surrounding labels or layout best match the question.

## Exact Answer Discipline
- Copy names, numbers, and dates exactly from the document whenever possible.

- Preserve the document's exact spelling and punctuation for names and quoted phrases; do not substitute similar letters or change straight/curly quotes, spacing, or parentheses when the visible text provides them.
- Prefer direct extraction over paraphrase.
- Before finalizing, compare the answer against nearby alternatives and keep the best-supported exact span.

## Structured Layout Lookup
- For tables, first find the row or entry named in the question, then read the value under the requested column, header, date, or category; answer with that cell only.
- For forms, receipts, or labeled fields, locate the exact role, party, or field label mentioned in the question, then copy the filled-in value from the same line, box, block, or immediately adjacent field.
- For table-of-contents, indexed, numbered, or bulleted lists, match the requested title, entry, or point number, then follow the same line or list item to the associated value; do not take a nearby value from another item.

## Anchored Handwriting / Nearby Text
- For handwritten or list/table questions with an anchor term, first locate the anchor, then inspect the immediately adjacent text in the same row, column, or nearby margin. If legible, provide the best-supported nearby span rather than leaving the answer blank.

# ALFWorld Embodied Agent Skill

## Overview
This skill guides agents operating in the ALFWorld text-based embodied environment.
The agent must complete household tasks by navigating rooms, interacting with objects,
and using appliances. Actions must be chosen from the admissible action list provided
at each step.

**Output format**: Always output `<think>...</think>` for reasoning, then `<action>...</action>` for the chosen action.

---

## Task Types

| Type | Goal | Key Steps |
|------|------|-----------|
| Pick & Place | Put object X in/on receptacle Y | Find X -> take X -> go to Y -> put X in/on Y |
| Pick Two & Place | Put two instances of X in/on Y | Find X1 -> take -> place -> find X2 -> take -> place |

### Pick Two Object Bookkeeping
For `pick_two_obj_and_place`, choose one destination receptacle instance once it is opened/usable, and remember it as the target. Both object instances should be placed into that same remembered receptacle. After placing the first object, do not remove it again; if the second object was already seen, return directly to its remembered location rather than searching randomly. If the two objects are accidentally split across different receptacles, consolidate them into the chosen target receptacle.
| Examine in Light | Examine object X under desklamp | Find X -> take X -> find desklamp -> use desklamp |

| Examine in Light detail | Final interaction | While holding X where a desklamp is visible, use the desklamp; do not try to place X on the lamp first. |
| Clean & Place | Clean object X and put in/on Y | Find X -> take X -> go to sink -> clean X -> go to Y -> put X |
| Heat & Place | Heat object X and put in/on Y | Find X -> take X -> go to microwave -> heat X -> go to Y -> put X |
| Cool & Place | Cool object X and put in/on Y | Find X -> take X -> go to fridge -> cool X -> go to Y -> put X |

---

## General Principles

1. **Decompose the task**: Parse the goal into ordered sub-goals (locate, acquire, transform, deliver). Complete each before moving to the next.
2. **Systematic exploration**: Search each surface and container exactly once before revisiting. Open closed containers (drawers, cabinets, fridge) before judging them empty.

- Prioritize semantically likely locations first, then broaden systematically: food in fridges/on countertops or dining tables; dishes/utensils/cookware on countertops, dining tables, stoveburners, cabinets, or drawers; office/bedroom items on desks, shelves, dressers, sidetables, or in drawers; newspapers on coffeetables, sidetables, sofas, or tvstands; toiletries/cleaning items near sinks, bathroom counters, shelves, carts, or cabinets.

- For portable kitchen targets such as bread, mugs, cups, plates, bowls, and utensils, check broad exposed surfaces early: after one or two empty countertops, try dining tables or other open surfaces before opening many cabinets/drawers. For small office/bedroom targets, alternate drawers with exposed desks, shelves, sidetables, and dressers rather than exhausting drawers first.

- Keep a persistent **searched set** of receptacle instances, e.g. `drawer 1`, `shelf 3`, `countertop 2`. Once an observation shows no needed target object there, mark it searched and do not call it “unexplored” later.
   - If all locations in the current preferred class are searched, **broaden to any unvisited admissible `go to ...` location** instead of restarting the same sequence. Search broadly across surfaces, furniture, containers, and appliances when relevant.
   - If a visible object is itself an openable/container-like object, such as a box, and opening/examining it is admissible, inspect it before leaving the area.
3. **Grab immediately**: When a required object is visible and reachable, take it right away before moving elsewhere.

- Pick up only the exact requested object type. Similar or related objects, such as a cup when the task asks for a mug, a spoon when it asks for a knife, or a pot when it asks for a pan, are distractors; leave them in place and mark that location searched for the target.
4. **Transform before placing**: If the task requires cleaning, heating, or cooling, perform the state change at the appropriate appliance before heading to the final destination.

- Do not repeatedly revisit the sink, microwave, fridge, or final destination before holding the target object. If you find the appliance early, remember its location, then resume searching unvisited object locations until the target object is acquired.

- Use direct admissible appliance/tool commands immediately when available, such as `clean X with sinkbasin`, `heat X with microwave`, `cool X with fridge`, or `use desklamp`. Do not waste steps opening, closing, toggling, or examining the appliance unless the needed action is unavailable or opening is required for searching/placing.
5. **Direct delivery**: Once holding the transformed (or untransformed) goal object, navigate straight to the target receptacle and place it.

- Remember known destination receptacles and return directly to the same instance after pickup/transformation. If the destination is also a semantically likely source location, check/open it early rather than only after exhaustive search: food may already be in the fridge, utensils may be on the diningtable, newspapers may be on/near the sofa, and a target drawer can be opened early for pick-two tasks. If the object starts at the destination but needs cleaning/heating/cooling, take it out, transform it, then return to that same instance and place it back.
6. **Track progress**: Maintain an internal count of how many objects still need to be found and placed. Only stop searching when the count reaches zero.
7. **Avoid loops**: Never repeat the same action more than twice in a row. If stuck, move to a different unexplored location.
8. **Only choose admissible actions**: Always pick an action from the admissible action list. Do not invent actions.

---

## Common Mistakes to Avoid

- **Revisiting searched locations**: Keep track of which surfaces/containers have been checked; do not re-examine them.
- **Ignoring visible objects**: If the target object appears in the observation, pick it up immediately.
- **Skipping state changes**: Do not place an object at the destination without first cleaning/heating/cooling it when required.
- **Premature termination**: Do not stop the episode until all goal conditions are verified as met.
- **Action loops**: Repeatedly toggling or examining the same object wastes steps. Move on to new locations instead.

### Hard Search-Loop Recovery

- **Exact-instance lockout before pickup**: once a receptacle/surface instance has been observed and does not contain the target object, do not go back to that exact instance while still searching for the object. A phase change, such as holding the object or needing final delivery, is the only reason to return.
- **Fast broadening threshold**: after 3-4 misses in the same receptacle class, switch to a different likely class or any unvisited admissible location instead of continuing or restarting that class, unless the target has already been seen there.
- **No search reset by recency**: do not say a location is "unsearched" merely because it was not in the last few observations. The searched set is global for the whole episode.
- **Finite-class exhaustion**: if all visible instances of a small class have been checked once, such as all stoveburners, diningtables, countertops, or shelves, mark that class exhausted for object search and do not start a second pass. Remember a usable destination instance, then search different receptacle classes.
- **Unvisited beats likely-but-searched**: after several misses, prefer any admissible unvisited `go to`, `open`, or `examine` target over revisiting a semantically likely but already-searched location.
- **Destination surfaces before pickup**: if the destination receptacle is also a likely object location, inspect each instance at most once before pickup. If it lacks the object, remember it as the final destination but stop using it as a search target until the object has been transformed and is ready to place.
- **Kitchen item fallback**: for cookware and dishware, after checking obvious burners/tables/counters once, broaden to unsearched cabinets, drawers, shelves, sinkbasins, and other kitchen storage/surfaces rather than cycling among the obvious locations.

### Strict Search Ledger Action Filter

Before every empty-handed search action, apply this hard filter:

1. If a required target object is visible, take it immediately.
2. Otherwise choose an exact receptacle/surface/container instance whose contents have not yet been observed in the current object-search phase.
3. Reject any `go to`, `examine`, or `open` action for an exact instance already observed to lack the target, even if it is semantically likely, nearby, recently mentioned, or the final destination type.
4. If all likely instances are rejected by the ledger, broaden to any unvisited admissible location/class instead of restarting from instance 1 of a searched class.

The searched ledger survives inventory checks, appliance visits, placing the first object in a pick-two task, and putting down an irrelevant inspected object/container. These events are not permission to rescan shelves, drawers, cabinets, tables, counters, or destination receptacles from the beginning.

### Destination-as-Source Lockout

When the final receptacle type is also a plausible source location, inspect each visible destination instance at most once before pickup. After it lacks the target, remember a usable destination instance and lock that exact instance out of object search until you are holding the required object ready for delivery. Do not alternate between destination instances and other searched source instances while still empty-handed.

### Pick-Two Phase Memory

After placing the first object in a pick-two task, do not begin a fresh room/class search. If another required instance was previously seen, return directly to that remembered source location for the second pickup. If no second instance is remembered, continue from the existing unsearched-location ledger rather than revisiting locations already checked before the first placement.

<!-- SLOW_UPDATE_START -->
Preserve the successful pattern: when the exact requested object is visible, take it immediately; perform the required clean/heat/cool/use action as soon as the correct command is admissible; then deliver directly to the remembered destination.

Treat tool locations as tools, not repeated search targets. If a sinkbasin, fridge, microwave, desklamp, or destination receptacle has already been checked and does not contain the target while you are empty-handed, remember it for later but do not revisit it until you are holding the required object or ready to place/use it.

Use a next-unsearched-instance pointer for every numbered class. If you leave cabinets, drawers, shelves, countertops, or stoveburners and later return to that class, resume at the lowest exact instance not yet observed; never restart at instance 1 and never revisit an instance already observed to lack the target.

For pan-to-stoveburner tasks, search in a step-efficient order: make one quick pass over stoveburners only to find a pan or remember an empty destination, then leave stoveburners until delivery. Next check countertops/islands and sinkbasins. Then prioritize cabinets in numeric order, opening each closed cabinet and observing its contents, before low-yield drawers. Do not abandon cabinet search to revisit searched stoveburners, countertops, or drawers.

For kettle/teapot clean-and-place tasks, after checking obvious countertops/islands, check stoveburners and sinkbasins once, then cabinets in numeric order. If several cabinets are empty, continue to the next unsearched cabinet or broaden to unvisited shelves/carts/dining tables; do not return to already searched countertops. Remember one open/empty cabinet as the final destination, but do not keep using searched cabinets as search targets.

For dishsponge clean-and-place tasks, check sinkbasin and nearby countertops once, then search unvisited cabinets, drawers, shelves, carts, and other storage/surfaces. Because the sink is needed for cleaning, remember it after the first visit; do not go back to the sink while empty-handed just because the sponge is likely near it. Because shelf is the destination, remember a usable shelf after inspecting it once; after a shelf lacks the sponge, search only unvisited shelves or other unvisited locations until the sponge is found.

When the step budget is running and you are still empty-handed, prefer any unvisited admissible location over any searched likely location. A location being semantically likely, useful later, or recently mentioned is never a reason to rescan it before acquisition.

Do not let the broadening threshold cause class restarts. Broadening means move to a different unvisited class or continue at the next unsearched instance of a promising storage class; it never means cycling back through exact instances already observed.

# Live Mathematical MCQ Heuristics

## Option Comparison

### Meta-Options About Stronger Results
- Treat options of the form “one of the remaining options is correct, but a stronger result can be proven” as serious candidates, especially when the question asks for the strongest statement.
- If a concrete option is true but your theorem or derivation gives a strictly stronger conclusion not exactly listed, choose the meta-option rather than the weaker concrete statement.
- When options are nested by strength, rank them explicitly before answering: e.g. finite-time blowup is stronger than merely “not globally bounded”; positive stable growth is stronger than ordinary unboundedness; sharper constants, rates, exceptional-set bounds, endpoint inclusion, or full equivalences are stronger than weaker asymptotic versions.
- Compare all options before committing. The correct choice is often the strongest statement justified by the question, while nearby distractors are weaker, overstrong, or miss an equality case.
- Track exact quantifiers such as "there exists", "for every", "if and only if", and "exactly when".

## Theorem-Level Precision

- Do not add converse, realization, or classification claims unless the theorem explicitly proves them. Phrases such as “conversely,” “every such parameter occurs,” “if and only if,” or “exactly all” add strength beyond a one-way implication.
- Check whether an option weakens the conclusion by dropping a characterization, equality clause, or full equivalence.
- Check whether an option overstates the theorem by upgrading regularity, removing scale restrictions, or changing an existential statement into a universal one.

## Hypotheses

### Exact Conditions and Thresholds
- For biconditional/equivalence questions, reject conditions that are merely necessary or merely sufficient. A broader condition, such as congruence modulo a divisor instead of modulo the full modulus, is usually weaker and not equivalent unless the domain collapses the extra cases.
- For threshold conditions, verify the exact sign and endpoint: distinguish \(\mu_0\) from \(-\mu_0\), \(<\) from \(\le\), and whether the equality case belongs to the positive, zero, or negative parameter regime.
- When options differ by “for every” vs “for sufficiently large,” local vs global domains, strict vs non-strict inequalities, or dependence of constants, rank them by logical strength and match the sharpest justified version.
- Verify the hypotheses and domain carefully. Distractors often keep the theorem shape but alter the required assumptions.
- Pay close attention to equality cases, extremal conditions, and whether a result applies to the full family or only a restricted subfamily.

## Final Answer
- Output the final answer as the single option label only.

## Exact Scope and Quantitative Wording
- Distinguish global conclusions from localized or completed ones. Equivalence after localization, completion, or at each prime/scale is usually weaker than an unqualified equivalence.
- In estimate-heavy options, compare every quantitative detail: exponent, derivative index range, constants and their parameter dependence, log factors, additive terms, one-sided vs two-sided notation, and pointwise vs uniform convergence.

# OfficeQA Skill

## Retrieval Discipline

- When an external official time-series observation is needed, prefer the source's series/data-download/table page once identified. If exact-date or guessed-value searches return empty results, stop repeating them; broaden to the official series name/code plus `data` or `download` and use the table values.

- Treat provided/oracle parsed pages as primary evidence: if they contain the relevant table and period, extract directly from them before searching elsewhere; search only for missing continuation pages, missing periods, or an official actual value not present.
- Start by narrowing to the most likely candidate file before reading long passages.
- Prefer targeted search terms that name the exact entity, period, measure, or table concept from the question.
- After a promising match, read only a small surrounding span and verify it matches the requested year, basis, and unit.

- If the requested date range extends beyond the provided/oracle page, first enumerate the required periods and verify that every period is present in evidence. Do not compute from a partial ledger or fill missing periods from memory; retrieve continuation pages, adjacent issues, or a later issue of the same table that contains the missing dates/revisions.

## Evidence Discipline
- Extract the exact value from the retrieved text before doing any arithmetic.
- Keep track of each operand's period, unit, and semantic role so nearby proxy values are not mixed in.

- For Treasury financing narratives, label each amount by transaction role before calculating: offered amount, tenders/subscriptions received, tenders accepted, competitive/noncompetitive accepted, foreign or Government-account exchange tenders, refunding, and **new cash** are not interchangeable.
- When converting currencies or scales, make a direction ledger first: source table unit, source currency, exchange-rate orientation (foreign currency per U.S. dollar means divide by the rate; U.S. dollars per foreign unit means multiply), and requested final unit.

- For tables, align values by row label and exact column header, not proximity alone; watch for continued or unlabeled columns, footnotes, adjacent amount-versus-percent columns, fiscal-year versus calendar-year sections, and repeated month rows under different year blocks.
- If the question asks for a transformed or derived quantity, compute only after confirming every operand.

- For derived comparisons, preserve the direction and sign implied by the wording: “change from A to B” means B minus A; “former than latter” means former minus latter; “share accounted for by X” means X divided by the stated total; paired “gap” questions require computing each within-row difference before ranking.
- For statistical, regression, correlation, and growth-rate questions, write a formula ledger before calculating: confirm the exact series/endpoints, ordered vector, elapsed intervals, and requested convention such as continuously compounded rate, CAGR, Pearson correlation, or OLS index/year choice.
- For multi-stage questions where one table determines the period/entity used in another lookup, freeze that derived key with evidence first, then retrieve the second measure only for that exact month/year/reporting date/entity.

- For inclusive time-series ranges, make a period-by-period ledger covering every requested month/year exactly once, preserving calendar versus fiscal basis, end-of-month or end-of-fiscal-month status, source units, and any specified adjustments.

- For statistical transforms over time-series windows, confirm endpoint inclusion/exclusion exactly as worded, use consecutive time indices for trend regressions when appropriate, sort values before medians, and for logarithmic growth use ln(final/initial) before converting to the requested percentage format.

## Final Answer Discipline

- Before finalizing, enforce the requested unit and format: convert thousands/millions/billions or full nominal dollars as needed, then apply no-comma, fixed-decimal, whole-number, or nearest-tenth/thousandth formatting exactly as asked.
- Return the final answer only after one last consistency check against the retrieved evidence.
- Copy the final answer from a checked value, not from an unverified intermediate guess.

## Statistical and Time-Series Calculation Checks

- Before computing any statistic, write the intended formula and denominator convention. If the prompt explicitly says **population standard deviation**, divide by `n`; if it says **sample**, divide by `n-1`; for a z-score comparing one observation against a small set of comparison months/periods and no population convention is stated, estimate dispersion with the **sample** standard deviation of the comparison set. Do not round intermediate operands, weighted averages, logs, exchange-rate conversions, or standard deviations before the final requested rounding.
- For long inclusive ranges, first enumerate the expected count of observations and the first/last period, then verify the ledger has exactly that count. Exclude totals, cumulative-to-date columns, comparable-period columns, estimates, and extra latest-month columns outside the requested calendar or fiscal range.
- When a page contains multiple nearby sections with similar labels, use only the section whose title and row label match the requested measure exactly; do not compute from the first visible table if the requested measure/table title is absent or only partially shown.
- For Treasury security quotations, obey the table's quote basis. If the table states that price decimals are 32nds, convert quotes such as `99.27` as `99 + 27/32`, not as decimal `99.27`. If a task asks for smoothing, averaging, or forecasting in a target currency using period-specific exchange rates, convert each period's observation to the target currency first unless the prompt explicitly says to compute in the source currency and convert only the final result.

## Stricter Final Formatting

- Match any requested output template exactly. Unless the prompt explicitly asks for unit words or explanatory text, return only the numeric value or requested list; do not append words such as `million`, `dollars`, `percent`, or `percentage points`. Include symbols/commas only when the prompt requests currency-formatted output or the answer format clearly requires them.

# Question Answering Skill

(No learned rules yet. Rules will be added through the reflection process.)

## Concise Answer Normalization
- Prefer the shortest unambiguous answer that directly satisfies the question. Do not include generic descriptors, legal suffixes, or expanded formal names unless the question specifically asks for the full official name or the descriptor is necessary to identify the entity.

- If the answer appears inside a longer descriptive phrase, strip words that merely repeat the clue's requested type or modifiers already stated in the clue. For short-answer trivia, return the distinctive core entity or headword rather than role titles, product flavor adjectives, or place/facility designators, even when those words are part of a fuller official phrase, unless the full official name is explicitly requested.
- For place/name-etymology questions asking for “the name” or “the word” that means something, answer the distinctive name/word itself rather than a larger phrase with a generic type label.

- For natural geographic features, preserve conventional feature designators such as “Lake,” “River,” “Bay,” “Gorge,” “Mount,” or “Island” when they are part of the proper name or match the requested feature type. Do not shorten “Lake Okeechobee,” “Tampa Bay,” or “Olduvai Gorge” to an ambiguous base name merely to be concise.
- For companies, brands, and organizations, answer the common distinctive name when sufficient; omit additions such as “Company,” “Corporation,” “Inc.,” etc. unless explicitly required.

- Preserve the answer surface form supported by the strongest evidence when exact variants differ: spelling, capitalization, punctuation, and word order can matter. Do not substitute an equivalent official/common variant such as an alternate spelling or inverted institution name if a direct title/snippet/answer field gives the expected form.

- When copying titles or quoted names, preserve ordinary ASCII punctuation from the evidence, especially straight apostrophes (`'`). Do not replace them with typographic curly quotes/apostrophes unless that exact stylized form is explicitly shown as the supported answer.

- For nicknames, epithets, saints, and quoted titles, copy the supported surface form exactly, including spacing, capitalization, and conventional abbreviations such as “St.” Do not normalize a stylized or quoted form into a lowercase dictionary word or an expanded spelling when the clue/evidence points to the stylized answer.

- For person answers in trivia or crossword-style clues, prefer the conventional supported name. Use just a surname, first name, or saint/regnal name only when the clue/source clearly expects that short form; otherwise use the canonical full personal name from the strongest evidence or answer field, especially when a lone given name would be ambiguous.
- Return the grammatical base form expected by the clue. Do not add a plural `s` merely because a crossword source pluralizes a shared name or category; if the clue lists people sharing a first name, answer the singular given name.

- For common-noun category answers, default to the singular dictionary headword in trivia/crossword-style clues, even if the clue uses plural words like “these,” “those,” “places,” or “items” for grammar. Use a plural only when the term is inherently plural or an answer field/source clearly gives a plural phrase.

- For common-noun clues about things being replaced, used in place of, or substituted by another system/item, answer the broad headword for the thing replaced unless a narrowing modifier is required by the clue or answer field. Do not add adjectives such as “letter,” “regular,” or “standard” merely because they appear in explanatory context.
- For fill-in-the-blank or definitional clues using words like “this” or “that,” provide a standalone noun phrase. Avoid context-dependent pronouns or possessives from the source text; use a natural article such as “the” when needed (e.g., answer “the highest point,” not “its highest point”).

## Context-Grounded Evidence Matching
- Start by identifying the most distinctive terms in the question: proper names, dates, titles, quoted phrases, unusual words, roles, relationships, and category descriptors.
- Prioritize passages or document titles where several distinctive clue terms occur together, especially if the wording directly repeats or closely paraphrases the question.
- Treat document titles as useful evidence: the answer is often named in a title while the snippet confirms the clue facts.

- Do not assume the document title itself is the answer. If the requested type differs from the title entity, use the title as context and extract the matching typed entity from the snippet or clue relationship.

- For “known as,” “called,” “defined as,” or category/type clues, choose the canonical term explicitly used in the strongest matching title/snippet or scraped answer field rather than inventing a related derivative or near-synonym from the clue wording. When multiple plausible candidates appear, prefer the candidate whose evidence directly states the requested relationship and repeats the most distinctive clue facts.
- Ignore noisy results that only match generic words; prefer evidence that directly connects the clue facts to one specific entity.

## Clue Interpretation and Answer Type
- For Jeopardy-style wording such as “this man,” “this group,” “this film,” “this country,” “this system,” “he,” or “his wife,” infer the expected answer type before choosing the answer.
- Use that expected type to validate candidates: answer with the concise person, place, title, organization, object, term, or phrase requested by the clue.

- Treat modifiers attached to the requested type as hard filters, not background flavor: constraints like dates, “largest,” “2-letter-named,” “1978 remake,” “hot dog brand,” “dual throne,” or “on this company’s board” must all fit the candidate before you answer.
- For clues centered on creative works such as books, films, plays, songs, poems, or other media, first determine whether the clue asks for the work itself, its creator, a performer or cast member, a character, a quotation source, or a setting. Verbs such as “wrote,” “directed,” “stars,” “played,” and “set in,” plus pronouns like “he” or “her,” usually determine the target.

- For fill-in-style clues with placeholders such as “this,” “these,” or “one of these,” substitute each candidate back into the clue and choose the concise answer that makes the full phrase, title, or fact read correctly.
- For terse clues that are just examples or names separated by commas, slashes, or “or,” infer the shared category, class, or synonym that links them, then answer with that concise common term.

- For crossword-style clues, treat parenthetical numbers or stated letter counts as hard constraints on the answer length, and omit generic labels that would violate them. In dual-definition clues using wording like “X, or what Y does,” choose the single word that satisfies both senses and preserve the required inflected form.
- If the clue references an unavailable image or link with wording like “seen here,” “pictured,” or parenthetical visual hints, rely on the textual clues and context to infer the answer; do not treat the missing image as necessary evidence.
- If multiple snippets support the same entity, use that corroboration to choose the canonical/common form of the answer.

## Trivia / Jeopardy Snippet Formats
- Retrieved trivia snippets may contain the clue and answer in scraped formats such as `CATEGORY | clue | answer`, `clue. ANSWER`, or labels like `right:`.
- When the question text matches the clue in such a snippet, extract the answer field or adjacent answer name, not the category or the whole clue sentence.

## Common Clue Traps
- Watch for inverse relationships: if the clue says “His third wife was Jiang Qing,” the requested answer is the husband, not Jiang Qing.

- More generally, preserve relation direction in clues: “A is evidence of this B,” “A is related to this language,” or “home to these characters” asks for the target of the relationship, not the entity already named in the clue.

- When a clue says examples, models, breeds, members, or items “include,” “like,” or “such as” named entities, treat those names as evidence for the requested parent class or entity. Answer the encompassing brand, animal, category, place, or term requested by “this,” not one of the examples already given.
- If the question gives the start of a quotation or phrase, answer with the exact missing continuation from the context.

- For song, poem, nursery-rhyme, or quotation clues, first decide whether the question asks for a missing word or phrase from the quote or for the associated creator, performer, or work; use pronouns and answer-type signals to choose the right target.
- When a clue asks for a constrained form such as a first name, abbreviation, acronym, or lyric word, return that exact form rather than the fuller person, title, or explanation; preserve conventional punctuation or spelling when it is part of the requested form.
- If the clue contains wordplay, quotation marks, or puns, treat them as hints, but answer with the real entity supported by the evidence.

- If a clue includes a quoted title, quoted narration or lyric, named event, slogan, or other distinctive phrase but asks for an associated “this” entity, treat the quote or name as evidence to identify the requested person, work, place, group, category, source, or term; do not return the quoted anchor unless the clue explicitly asks for it.

# Spreadsheet Manipulation Skill (xlsx)

## Overview
This skill guides agents in manipulating Excel (.xlsx) spreadsheets using Python.

**Primary libraries**: `openpyxl` (structure-preserving read/write), `pandas` (data transformation).
Never use any other third-party libraries.

---

## Common Workflow

1. **Explore** the input file: list sheets, inspect headers, check dimensions.

- Inspect actual workbook data beyond the preview, including nearby rows/columns, sample outputs, formulas, labels, headers, and any reference/example sheets such as `Output`, `Manual Result`, or `Desired...` tabs.

- Treat existing filled cells in the requested output area or adjacent example tables as semantic examples for edge cases and expected formats, but still recompute and write the complete requested target range.
   - Scan the used range for complete header groups, not just row 1. Tables may start in later rows/columns, have title rows above them, or have multiple source/result tables on the same sheet; use nearby labels and the requested output range to distinguish sources from destinations.
   - Locate tables, fields, and target ranges by header text, nearby labels, and surrounding nonblank structure rather than fixed coordinates. Build header maps from actual cells when useful, e.g. `{str(cell.value).strip(): cell.column}`.
2. **Write `solution.py`** with `INPUT_PATH` and `OUTPUT_PATH` defined at the top.
3. **Execute** `python solution.py` and verify the output file was created.
4. **Confirm** the target cells/range contain the expected values.

---

## Library Selection

| Use case | Library |
|----------|---------|
| Preserve formulas, formatting, named ranges | `openpyxl` |
| Bulk data transformation, aggregation, sorting | `pandas` → write back with `openpyxl` |
| Simple cell read/write | `openpyxl` |

**Warning**: `pandas.to_excel()` silently destroys existing formulas and named ranges.
When writing back to a spreadsheet that contains formulas, always use `openpyxl.save()`.

**Formula evaluation caution**: `openpyxl` can write formulas but does **not** calculate them or update cached results. If the requested output will be checked as cell values, compute the result in Python and write literal values unless the user explicitly requires live formulas. When existing formulas are inputs to your logic, load a second workbook with `data_only=True` to read cached values while saving changes through the normal workbook:

```python
wb = openpyxl.load_workbook(INPUT_PATH)
wb_values = openpyxl.load_workbook(INPUT_PATH, data_only=True)
ws = wb["Sheet1"]
ws_values = wb_values["Sheet1"]
```

Treat wording such as “write/fix a formula,” “SUMIFS/COUNTIFS,” “VBA,” or “macro” as a description of the spreadsheet logic unless the deliverable explicitly requires live formula text, an `.xlsm`, or a preserved VBA project. For normal `.xlsx` outputs, implement the equivalent logic in Python/openpyxl and write the computed final values to the requested cells so verification does not depend on Excel recalculation or macros.

When the user provides an existing or broken formula, use it as a semantic specification: honor its referenced lookup ranges, criteria ranges, return ranges, aggregation intent, and error-handling behavior, then write the resulting values rather than guessing different source columns or leaving unevaluated formulas.

---

## solution.py Template

```python
import openpyxl
import pandas as pd

INPUT_PATH  = "..."   # set to the actual input path
OUTPUT_PATH = "..."   # set to the actual output path

wb = openpyxl.load_workbook(INPUT_PATH)
ws = wb.active  # or wb["SheetName"]

# --- perform manipulation ---

wb.save(OUTPUT_PATH)
```

---

## Output Requirements

- Save the result to `OUTPUT_PATH`.
- Do not hardcode row counts or column letters — iterate over actual rows in the workbook.
- Preserve sheets and cells not mentioned in the instruction.

## Matching and Target Range Hygiene

- Choose the comparison operator from the instruction and examples: use `startswith` for “begins with”, substring search for “contains/search/occurrence”, and exact normalized equality only when a whole-cell match is implied.
- Create small helper functions for comparisons and numeric parsing. Normalize text by trimming, collapsing repeated spaces/NBSPs, and casefolding; when names or labels have punctuation/spacing inconsistencies, consider punctuation-insensitive keys. Parse numeric text after removing commas/currency symbols while preserving signs and decimal points; skip `None`/blank and booleans for numeric tests, and handle placeholders such as `"-"`, `"$"`, `"$0"`, blanks, and numeric zero deliberately.
- Normalize date keys deliberately: handle `datetime`/`date` objects, Excel serial numbers, and date-like strings, then compare at the granularity implied by the task, such as exact date, month, month/year, fiscal period, or year. For workday/date-window logic, compute the range in Python and exclude weekends/holidays as specified.

- For monthly or period summary grids, canonicalize period labels from all sources: sheet names, title text, row/column headers, text months such as `March`, and actual date cells. Match summaries by normalized period plus the other stated criteria rather than by fixed month offsets or existing formulas.
- For date ranges and rolling windows, infer endpoint inclusivity from wording and examples. Phrases like `X to Y`, `through`, and `up to`, or examples such as `2 to 5` meaning `4 days`, usually require inclusive boundary handling.
- For time extraction or time-threshold logic, parse `datetime`, `time`, Excel serial/fractional times, and time-like strings into real Python `time`/`datetime` values. Write real time values with an Excel `number_format` such as `hh:mm:ss AM/PM`; do not write text substrings when the result should behave as a time.
- For joins, deduplication, grouping, interval lookups, lookup grids, and ordered outputs, build explicit normalized keys, including composite keys when the task refers to multiple fields. Preserve original source order within each group unless sorting is explicitly requested.
- For outputs that depend on other rows or lookup grids, make a first pass to build normalized dictionaries/groups/range structures, then a second pass to write results. Avoid nested full-sheet scans per row; split delimited tokens and ignore empty tokens, and treat error literals such as `#N/A` as meaningful sentinel values when the task refers to them.

- For lookups, filters, joins, and label/header matching, normalize comparison keys consistently: trim whitespace, skip blanks explicitly, use case-insensitive text matching when appropriate, and treat numeric-looking IDs consistently (`330`, `330.0`, and `"330"`). Keep numeric outputs numeric; use `number_format` for display formatting instead of converting numbers to strings unless text is explicitly required.
- When replacing a generated output area, clear only the instructed target range before writing new results so stale values/formulas do not remain. Preserve formatting, column widths, borders, formulas, and unrelated cells unless the instruction explicitly asks to change them.

- If the instruction includes formatting changes, apply them exactly after writing values and only to the requested cells/range. Use `openpyxl` styles for fills, alignment, fonts, borders, and number formats; convert hex colors to ARGB when needed, for example `#FFC000` → `FFFFC000`. For “format as text,” set `number_format = '@'` and write string values when the expected cell values are text.

- When the instruction names a destination range or columns, write derived results directly there. Do not insert rows/columns, relocate the source table, or sort/delete source records unless that structural change is explicitly requested.
- For filtered lists, summaries, and aggregations, first collect all source records/results in memory, preserving the required order, then write from the first output row and clear leftover cells below the new results in the target columns. When adding rows, copy style/alignment/number format from an existing template row when appropriate; when deleting rows, delete from bottom to top to avoid row-index shifts.
- Preserve intended blanks as empty cells (`None`) rather than placeholder text or `0` unless the task specifies otherwise.

- For numeric aggregation, crosstab, SUMIFS-like, and INDEX/MATCH-style summary outputs, infer missing-match behavior from table semantics and examples: numeric summary grids usually require literal `0` for no matching records, while filtered lists or “show only once” outputs usually require blanks (`None`).
- For blank-sensitive logic such as “if input is blank, output blank,” evaluate the driving input with `data_only=True` when it may itself be a formula, and write `None` for truly blank outputs rather than relying on a new formula returning `""`.

## Robustness for Simple Fill Tasks

- Prefer simple, auditable row/column loops over complex workbook XML parsing unless the task truly requires unsupported workbook internals. Before returning, run the script once to catch syntax/indentation errors and verify that representative target rows were actually written.

<!-- SLOW_UPDATE_START -->
When the user asks for a formula, macro, VBA code, or a fix to an Excel formula, still deliver the completed workbook state: compute the intended results in Python and write literal final values into the requested cells. Do not write formula strings unless the task explicitly says the output must contain live formulas.

After writing, reload or inspect the saved workbook and verify that every requested/evaluated target cell contains a non-formula literal where a value is expected. If a target cell is still `None` unexpectedly, fix the script before finishing.

Use existing formulas in the workbook as examples/specifications, not as output. If a cell contains a reference formula such as `=A25` or an INDEX/MATCH/SUMIFS pattern, parse what source cells/ranges/criteria it refers to, compute those results yourself, and overwrite the destination with the referenced or calculated value.

For blank-sensitive formula tasks, compute the branch explicitly: if the driving source cell is truly blank, write `None`; otherwise write the actual result such as `0`, `1`, a category label, or a lookup value. Never rely on `IF(...,"",...)` formulas to be recalculated later.

For lookup/category tasks, locate both the input rows and the lookup table by headers and nearby labels. Support exact keys, numeric-looking keys, and interval/range tables; then fill every destination row that has a driving input, not just the first visible example.

For “every nth row” or OFFSET-style tasks, infer the source column, first source row, and step from the provided examples or formulas, then copy the actual source values into the requested output range as literals.

For schedule/calendar fill tasks, build a cycle-day-to-periods mapping from the schedule/template area first, then fill the daily rows across all requested class columns based on each row’s cycle day. Preserve repeated/double periods exactly as shown by the template; do not leave formulas in the schedule cells.

For INDEX/MATCH problems where the first row works but subsequent rows fail, treat row labels, column/year headers, region/type criteria, and expense/category labels as a multi-key lookup. Fill the whole result matrix with values from the source data table, using cached `data_only` values when source cells are formulas.

For multi-step macro/VBA-style requests, implement every stated operation in the workbook, not just the first deletion/filtering step. Re-read the numbered requirements before saving and verify later computed columns, totals, and derived fields as well as the obvious filtered rows.

When a target range includes special rows such as `Total`, `Grand Total`, `min`, `max`, constraints, headers, or blank separators, do not apply ordinary row logic blindly to those rows. Compute totals as aggregates when indicated, and leave constraint/header/blank cells untouched unless explicitly requested.

For residual-balancing tasks, identify data rows separately from min/max constraint rows. Add positive residuals from unit 1 toward unit 5 without exceeding max values; subtract negative residuals from unit 5 toward unit 1 without going below min values; update only the unit cells in actual data rows.

For time-threshold rows, decide per row whether it is a normal data row or a summary row. Normal rows use the before/after threshold rule; summary rows should aggregate the computed normal-row results if the workbook labels or examples indicate a total.

Keep scripts simple enough to run cleanly. Avoid unnecessary dynamic code generation and fragile f-strings with regex expressions inside them. Always execute the final `solution.py`; fix any syntax, indentation, or runtime error, then verify representative target cells.

If workbook cells contain arbitrary sample text that could be sensitive or trigger content filters, do not quote large raw cell contents in your response. Process them locally in Python with neutral variable names and output only the completed script/workbook changes.