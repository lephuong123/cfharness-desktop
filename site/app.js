// Highlight the visitor's OS and show the latest version. Purely cosmetic: links work without JS.
(function () {
  var ua = navigator.userAgent || "";
  var win = /Windows/i.test(ua);
  var mac = /Macintosh|Mac OS X/i.test(ua) && !/iPhone|iPad/i.test(ua);
  var first = win ? "card-win" : mac ? "card-mac" : null;
  if (first) {
    var el = document.getElementById(first);
    el.classList.add("primary");
    el.parentNode.insertBefore(el, el.parentNode.firstChild);
  } else {
    document.getElementById("card-mac").classList.add("primary");
    document.getElementById("card-win").classList.add("primary");
  }
  fetch("https://api.github.com/repos/lephuong123/cfharness-desktop/releases/latest")
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (j) {
      if (j && typeof j.tag_name === "string" && /^v\d+\.\d+\.\d+$/.test(j.tag_name)) {
        document.getElementById("ver").textContent = "Bản mới nhất: " + j.tag_name + ".";
      }
    })
    .catch(function () {});
})();
