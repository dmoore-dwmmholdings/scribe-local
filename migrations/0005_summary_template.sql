-- Summary templates: which template framed the stored summary.
--
-- The template only changes the prompt sent to the LLM (general meeting,
-- standup, interview, 1:1, lecture, sales call); the summary JSON shape is
-- unchanged. NULL means a summary written before this column existed (treat as
-- the default "general" template).
ALTER TABLE summaries ADD COLUMN template text;
