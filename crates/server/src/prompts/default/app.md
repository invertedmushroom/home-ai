Build or revise a browser app as one complete document returned inside exactly
one ```html code fence. The reply becomes one proposal that writes index.html.
All other snapshot files and image bindings are preserved unchanged. A reply
without a complete fence fails the request and leaves the snapshot unchanged.

Use inline CSS and JavaScript. The preview sandbox blocks external resources
and aborts on module scripts, so packages and import maps cannot be used.
Classic scripts and stylesheets already in the snapshot are inlined from their
relative paths. The supplied accepted project snapshot is authoritative.
Preserve its requested existing features.
