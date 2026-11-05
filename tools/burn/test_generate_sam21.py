import hashlib
import os
import tempfile
import unittest
from pathlib import Path

from generate_sam21 import check_source, parse_args


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


class CheckSourceTests(unittest.TestCase):
    """Only the pinned bytes are ever handed to the converter, read once and bounded."""

    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.folder = Path(scratch.name) / 'src'
        self.into = Path(scratch.name) / 'tmp'
        self.folder.mkdir()
        self.into.mkdir()
        self.data = b'onnx bytes'
        self.sha = hashlib.sha256(self.data).hexdigest()

    def test_the_pinned_file_is_copied_and_the_copy_returned(self):
        (self.folder / 'encoder.onnx').write_bytes(self.data)
        copy = check_source(self.folder, 'encoder', len(self.data), self.sha, self.into)
        self.assertEqual(copy, self.into / 'encoder.onnx')
        self.assertEqual(copy.read_bytes(), self.data)

    def test_a_longer_file_is_refused_after_one_byte_past_the_pin(self):
        (self.folder / 'encoder.onnx').write_bytes(self.data + b'x' * 4096)
        with self.assertRaises(SystemExit):
            check_source(self.folder, 'encoder', len(self.data), self.sha, self.into)
        self.assertEqual((self.into / 'encoder.onnx').stat().st_size, len(self.data) + 1)

    @unittest.skipUnless(hasattr(os, 'mkfifo'), 'needs FIFOs')
    def test_a_pipe_is_refused_without_waiting_for_a_writer(self):
        os.mkfifo(self.folder / 'encoder.onnx')
        with self.assertRaises(SystemExit):
            check_source(self.folder, 'encoder', len(self.data), self.sha, self.into)
