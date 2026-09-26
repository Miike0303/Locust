import unittest
from qa_replay_oracle import assert_reextracted


def row(identifier, text, path="a.locres", **metadata):
    return {"id": identifier, "file_path": "C:/fixture/" + path, "source": text,
            "translation": text, "metadata": metadata}


class OracleTests(unittest.TestCase):
    def check(self, expected, actual, engine="unreal"):
        return assert_reextracted(engine, expected, "C:/fixture", actual, "C:/fixture")

    def test_rejects_swapped_resources_even_with_identical_text_set(self):
        with self.assertRaises(AssertionError):
            self.check([row("one", "Hola"), row("two", "Adiós")], [row("one", "Adiós"), row("two", "Hola")])

    def test_rejects_missing_duplicate_text_resource(self):
        with self.assertRaises(AssertionError):
            self.check([row("one", "Hola"), row("two", "Hola")], [row("one", "Hola")])

    def test_rejects_wrong_file_and_extra_backup_rows(self):
        original = [row("one", "Hola")]
        for extracted in ([row("one", "Hola", "b.locres")], original + [row("one", "Hola", ".locust/backup/a.locres")]):
            with self.assertRaises(AssertionError):
                self.check(original, extracted)

    def test_html_offset_growth_keeps_element_order_and_kind(self):
        original = [row("a#html:10", "Hola", "a.html", html_start=10, html_kind="text:p"), row("a#html:20", "Adiós", "a.html", html_start=20, html_kind="text:p")]
        extracted = [row("a#html:10", "Hola", "a.html", html_start=10, html_kind="text:p"), row("a#html:99", "Adiós", "a.html", html_start=99, html_kind="text:p")]
        self.assertEqual(self.check(original, extracted, "html-game")["resources_verified"], 2)
        extracted[1]["source"] = "Hola"
        with self.assertRaises(AssertionError):
            self.check(original, extracted, "html-game")

    def test_rpg_growth_changes_offsets_but_not_dialogue_identity(self):
        original = [row("Map#event_1#page_0#cmd_0#msg", "Hello there"), row("Map#event_1#page_0#cmd_2#msg", "Goodbye")]
        extracted = [row("Map#event_1#page_0#cmd_0#msg", "Hello\nthere"), row("Map#event_1#page_0#cmd_3#msg", "Goodbye")]
        self.assertEqual(self.check(original, extracted, "rpgmaker-mv")["normalized_rpg_dialogues"], 1)
        extracted[1]["id"] = "Map#event_2#page_0#cmd_3#msg"
        with self.assertRaises(AssertionError):
            self.check(original, extracted, "rpgmaker-mv")

    def test_rpg_choices_share_command_but_keep_choice_identity(self):
        original = [row("Map#event_1#page_0#cmd_2#choice_0", "Sí"), row("Map#event_1#page_0#cmd_2#choice_1", "No")]
        self.assertEqual(self.check(original, original, "rpgmaker-mv")["resources_verified"], 2)
        with self.assertRaises(AssertionError):
            self.check(original, [original[0], original[0]], "rpgmaker-mv")


if __name__ == "__main__":
    unittest.main()
