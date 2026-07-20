-- Verb filetype + tree-sitter integration for nvim-treesitter
-- (https://github.com/nvim-treesitter/nvim-treesitter, classic/`master`
-- branch API: `nvim-treesitter.configs`, `nvim-treesitter.parsers`).
--
-- The Verb grammar itself lives in ../tree-sitter-verb (a normal
-- tree-sitter grammar package: grammar.js, pre-generated src/parser.c,
-- and queries/{highlights,locals,indents}.scm). This file registers that
-- local grammar with nvim-treesitter's parser registry and tells Neovim
-- that `*.verb` files are filetype `verb`. Query files are exposed to
-- Neovim's runtime query loader via ../queries/verb/*.scm, which are
-- symlinks back into ../tree-sitter-verb/queries/ (single source of
-- truth — edit the grammar's copies, not these).
--
-- This directory is meant to be loaded as a lazy.nvim "local plugin"
-- (dir = this repo's editors/nvim), depending on nvim-treesitter so it's
-- guaranteed to already be on the runtimepath and loaded when this file
-- runs. See ../README.md and the sibling plugin spec this integration
-- ships for the user's config.
--
-- One-time setup after adding this to the runtimepath: `:TSInstall verb`
-- (needs a C compiler on PATH; the parser source is pre-generated, so
-- neither Node nor the tree-sitter CLI are required for this step).

vim.filetype.add({ extension = { verb = "verb" } })

local ok, parsers = pcall(require, "nvim-treesitter.parsers")
if not ok then
  return
end

-- Resolve editors/tree-sitter-verb relative to this file's location, so
-- registration works no matter where the compiler repo is checked out.
local this_file = debug.getinfo(1, "S").source:sub(2)
local nvim_dir = vim.fs.dirname(vim.fs.dirname(this_file)) -- editors/nvim
local grammar_dir = vim.fs.joinpath(vim.fs.dirname(nvim_dir), "tree-sitter-verb")

parsers.get_parser_configs().verb = {
  install_info = {
    url = grammar_dir, -- local directory: installed in place, not git-cloned
    files = { "src/parser.c" },
  },
  filetype = "verb",
}
