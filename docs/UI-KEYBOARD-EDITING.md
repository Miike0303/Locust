# Keyboard editing in the string table

Open a project in the Editor, then Tab past the sortable column headers to reach the table's current row.

| Key or action | Result |
| --- | --- |
| Up / Down arrow | Move focus within the current page, stopping at its ends. Does not open details or change the selected row. |
| Enter on a row | Open that row in the detail panel. The selected-row highlight and screen-reader selection describe this opened row, not keyboard focus. |
| F2 on a row | Edit that row's translation inline without changing the opened detail panel. |
| Enter in the inline field | Insert a newline. |
| Ctrl+Enter in the inline field | Save to the project and leave inline editing. |
| Escape in the inline field | Cancel changes made since entering this inline session and return focus to its row. An earlier draft is preserved. |
| Leave the inline field | Save to the project. Clicking the translation text also starts inline editing. |

A focus ring marks the row you are navigating; it can differ from the selected-row highlight. Sorting applies only to the current page and preserves focus by row identity. If that row is no longer on the page, the first visible row becomes the table's Tab stop. Use the named previous/next-page controls to change pages.

Drafts remain scoped to their project and entry. Inline editing is unavailable without a project or while that entry is saving. Failed saves retain the draft and offer Retry. A translation conflict requires an explicit **Load latest** or **Overwrite** choice; ordinary blur does not overwrite it. Storage warnings mean you should keep the window open and save your text to the project before closing it.

Row navigation and F2 do not intercept keys from nested controls or modified row shortcuts. Existing application shortcuts remain separate; inline Escape cancels the edit rather than closing the detail panel. The table caption and inline field provide English/Spanish keyboard hints matching the interface language.
