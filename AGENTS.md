# LocusFS Review And Refactor Prompt

Use this prompt as the standing reference for the current review and refactor work.

Spawn multiple subagents to cover the whole codebase in a top down fashion -- most public facing API should be reviewed first (watch) all the way down to the binary and concrete plugin implementations.

For each unit of review, cover the following grounds:

- API. Is the API clean, concise and easy to understand? Is it generic enough? Does it wrongfully bring implementation details to the surface? How easy is it to replace the implementation without changing the API?
- Redundancy. How many redundant structs/functions are created/duplicated across the codebase that duplicate same concepts? Maybe some functions repeat the same code over and over -- maybe a helper is needed? Is the number of layers of abstractions reasonable? How can we meaningfully shrink and simplify the code?
- Performance. Is the memory footprint reasonable? Do we perform redundant allocations? Do we have caches in hot paths where it can meaningfully help performance? Is the threading ok, is the code async enough and would it work well under high contention? Are the communication boundaries correct, like are we holding redundant locks where better concurrency model could be used?
- Tidiness. Is the code well documented where it matters? Is it structured well according to rust-guide?
- Best Practices. Are we not reimplementing something that is already made for us in other crates/by other apps? Can things be reused to reduce our code footprint and performance?
- Domain-specific. LocusFS file structure should be easy to read and maintain. Example: current `@property` and `@method` in the dbus plugin is hard to maintain, and should be replaced with separate `/dbus/<service>/methods` paths, where every non-dir in this path is a callable method, and `/dbus/<service>/objects` paths, where every non-dir in this path is a property of an object it belongs to. Also review whether all plugins have a standard pattern/structure and whether it is well documented.

For each review unit, agents should write a detailed report/plan into a separate set of Markdown files.

After that, read the findings and ideas, arbitrate, tweak and expand on the concepts, and edit files where needed. Do not rely only on context and memory; write big decisions down.

After that, execute the refactoring. Keep a brief log with timestamps on important milestones. Include a separate section on important decisions that were made during implementation. At the end of the log, prepare a section with questions for validating the viability of the made decision/implementation approaches.

Everything should be covered with tests, and tests should pass for the whole project.

Do not hesitate to use web to verify best practices, patterns, crates, or whatever else might be needed to enrich decision quality.

If needed, `../locus-shell` can be inspected as a consumer of the watch API, but it is not final and is just one consumer. It should not be the sole decision influencer.

Do not stop until confident in the decisions. Spawn subagents when needed for decision making process to provide a second or third look.

This pass is not interactive. The coordinator is the decision maker for the review and implementation, and user feedback is expected at the end after the implementation log and validation questions are prepared.
