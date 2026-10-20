// ButtercupQL's colours in the book: its steps and functions as well as Python's words. The book's code blocks of
// ButtercupQL are marked as Python, so that other Markdown readers colour them too; here they get their own.
(function () {
  "use strict";
  if (typeof hljs === "undefined") {
    return;
  }
  hljs.registerLanguage("buttercupql", function (hljs) {
    var STRING = {
      className: "string",
      variants: [
        { begin: /[fF]?"""/, end: /"""/ },
        { begin: /[fF]?'''/, end: /'''/ },
        { begin: /[fF]?"/, end: /"/, illegal: /\n/, contains: [hljs.BACKSLASH_ESCAPE] },
        { begin: /[fF]?'/, end: /'/, illegal: /\n/, contains: [hljs.BACKSLASH_ESCAPE] },
      ],
    };
    return {
      name: "ButtercupQL",
      keywords: {
        keyword: "and or not in is if else def import as on",
        literal: "True False None",
        built_in:
          "count count_distinct sum mean avg min max any all row_number rank dense_rank lag lead first last len abs " +
          "round str int float now today date datetime days hours minutes seconds weeks literal",
      },
      contains: [
        hljs.HASH_COMMENT_MODE,
        // A step: a word at the start of a line, or after `;`.
        {
          className: "keyword",
          begin: /(?:^|;)[ \t]*(?:from|where|select|del|sort|take|skip|distinct|summarize|pass|group[ \t]+by|(?:left[ \t]+)?join)\b/,
          relevance: 0,
        },
        STRING,
        { className: "number", begin: /\b\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?\b/, relevance: 0 },
        { className: "symbol", begin: /`[^`\n]*`/, relevance: 0 },
        { className: "title", begin: /(?<=\bdef[ \t]+)[A-Za-z_]\w*/, relevance: 0 },
      ],
    };
  });
  var blocks = document.querySelectorAll("code.language-python");
  for (var i = 0; i < blocks.length; i++) {
    blocks[i].innerHTML = hljs.highlight("buttercupql", blocks[i].textContent, true).value;
  }
})();
