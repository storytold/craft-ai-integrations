import unittest

from generate_sam21 import parse_args


class ParseArgsTests(unittest.TestCase):
    """A mistyped option must stop the run before anything is written."""

    def test_the_folder_alone_means_write(self):
        """Without a flag the run writes, and the folder is the one argument."""
        self.assertEqual(parse_args(['models']), ('models', False))

    def test_check_is_read_only_and_accepted_on_either_side_of_the_folder(self):
        """`--check` before or after the folder both mean compare only."""
        self.assertEqual(parse_args(['models', '--check']), ('models', True))
        self.assertEqual(parse_args(['--check', 'models']), ('models', True))

    def test_a_mistyped_option_is_refused_instead_of_falling_back_to_writing(self):
        """`--chek` used to be dropped, which selected the run that deletes and rewrites the tree."""
        for bad in (['models', '--chek'], ['models', '--check', '--force'], ['--chek', 'models']):
            with self.subTest(bad=bad):
                with self.assertRaises(SystemExit) as raised:
                    parse_args(bad)
                self.assertEqual(raised.exception.code, 1)

    def test_exactly_one_folder_is_required(self):
        """No folder, or two, is a usage error."""
        for bad in ([], ['--check'], ['a', 'b']):
            with self.subTest(bad=bad):
                with self.assertRaises(SystemExit) as raised:
                    parse_args(bad)
                self.assertEqual(raised.exception.code, 1)


if __name__ == '__main__':
    unittest.main()
