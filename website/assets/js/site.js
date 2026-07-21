/* Duckle website behavior: theme toggle, GitHub star count, mobile nav. */
(function () {
    "use strict";

    var root = document.documentElement;

    /* ---- theme toggle (persisted; default dark, set pre-paint in <head>) ---- */
    var toggle = document.getElementById("themeToggle");
    if (toggle) {
        toggle.addEventListener("click", function () {
            var next = root.getAttribute("data-theme") === "light" ? "dark" : "light";
            root.setAttribute("data-theme", next);
            try { localStorage.setItem("duckle-theme", next); } catch (e) {}
        });
    }

    /* ---- mobile nav ---- */
    var navToggle = document.getElementById("navToggle");
    var navLinks = document.getElementById("navLinks");
    if (navToggle && navLinks) {
        navToggle.addEventListener("click", function () { navLinks.classList.toggle("open"); });
        navLinks.addEventListener("click", function (e) {
            if (e.target.tagName === "A") navLinks.classList.remove("open");
        });
    }

    /* ---- GitHub star count ----
       duckdb.org renders a static build-time count; we render a "★" fallback and
       upgrade it to the live number via the public API, cached for an hour so we
       do not hammer the rate limit on every page view. */
    var REPO = "slothflowlabs/duckle";
    var countEl = document.getElementById("ghCount");
    function fmt(n) {
        if (n >= 1000) return (n / 1000).toFixed(n >= 10000 ? 0 : 1).replace(/\.0$/, "") + "k";
        return String(n);
    }
    function showStars(n) {
        if (countEl) countEl.textContent = "★ " + fmt(n);
    }
    if (countEl) {
        var cached = null;
        try { cached = JSON.parse(localStorage.getItem("duckle-stars") || "null"); } catch (e) {}
        var fresh = cached && (Date.now() - cached.t < 3600000);
        if (cached && typeof cached.n === "number") showStars(cached.n);
        if (!fresh) {
            fetch("https://api.github.com/repos/" + REPO, { headers: { Accept: "application/vnd.github+json" } })
                .then(function (r) { return r.ok ? r.json() : null; })
                .then(function (d) {
                    if (d && typeof d.stargazers_count === "number") {
                        showStars(d.stargazers_count);
                        try { localStorage.setItem("duckle-stars", JSON.stringify({ n: d.stargazers_count, t: Date.now() })); } catch (e) {}
                    }
                })
                .catch(function () { /* keep fallback */ });
        }
    }

    /* ---- dismissible announcement bar (per-version, like duckdb.org) ---- */
    var ann = document.getElementById("announce");
    var annX = document.getElementById("announceX");
    if (ann && annX) {
        var annVer = ann.getAttribute("data-v") || "1";
        try { if (localStorage.getItem("duckle-announce") === annVer) ann.style.display = "none"; } catch (e) {}
        annX.addEventListener("click", function () {
            ann.style.display = "none";
            try { localStorage.setItem("duckle-announce", annVer); } catch (e) {}
        });
    }

    /* ---- docs sidebar: mark the current page active ---- */
    var here = location.pathname.split("/").pop() || "index.html";
    document.querySelectorAll(".docs-side a").forEach(function (a) {
        var href = (a.getAttribute("href") || "").split("/").pop();
        if (href === here) a.classList.add("active");
    });

    /* ---- contact + connector modal delivery ----
       When FORM_ENDPOINT is set, submissions are POSTed to it and emailed
       server-side (Formspree-style), so the visitor never leaves the page and
       no mail client opens. When it is blank, the success screen offers the
       prefilled email as an optional, user-clicked link. Never auto-launches
       the mail app. */
    var HOST = "souravroy7864@gmail.com";
    // Direct server-side email via Web3Forms: the POST is emailed to the address
    // registered against this access key, so submit sends the message with no
    // mail client. The key is a public, submit-only key (safe in client JS).
    // Leave FORM_ENDPOINT "" to fall back to the optional-mailto success screen.
    var FORM_ENDPOINT = "https://api.web3forms.com/submit";
    // Hidden fields sent with every submission. Web3Forms requires access_key.
    var FORM_EXTRA = { access_key: "b77df62d-c53e-4d26-9ea8-ef9f4fa5c505" };

    var MODAL_CHK = '<span class="chk"><svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg></span>';

    // Render a final screen into a modal and wire its ".js-modal-done" control.
    function modalDone(modalEl, html, close) {
        modalEl.innerHTML = html;
        var d = modalEl.querySelector(".js-modal-done");
        if (d) d.addEventListener("click", function (ev) { ev.preventDefault(); close(); });
    }
    function sentScreen(msg) {
        return '<div class="modal-ok">' + MODAL_CHK + '<h3>Thanks - message sent</h3>'
          + '<p class="muted">' + msg + '</p>'
          + '<button type="button" class="btn btn-primary btn-pill js-modal-done">Done</button></div>';
    }
    function mailtoScreen(mailto, heading) {
        return '<div class="modal-ok">' + MODAL_CHK + '<h3>' + heading + '</h3>'
          + '<p class="muted">We will get back to you by email. To send now, open a prefilled message, or write to us any time at <a href="mailto:' + HOST + '">' + HOST + '</a>.</p>'
          + '<a class="btn btn-primary btn-pill" href="' + mailto + '">Open prefilled email</a>'
          + '<p class="modal-alt"><a href="#" class="js-modal-done">No thanks, close</a></p></div>';
    }
    // POST the submission to FORM_ENDPOINT (emails it server-side, no mail
    // client) when configured; otherwise show the optional-mailto fallback. A
    // network/endpoint failure also falls back so the message is never lost.
    function deliverForm(modalEl, submitBtn, fields, mailto, heading, sentMsg, close) {
        if (!FORM_ENDPOINT) { modalDone(modalEl, mailtoScreen(mailto, heading), close); return; }
        var body = new FormData();
        Object.keys(FORM_EXTRA).forEach(function (k) { body.append(k, FORM_EXTRA[k]); });
        Object.keys(fields).forEach(function (k) { if (fields[k]) body.append(k, fields[k]); });
        if (submitBtn) { submitBtn.disabled = true; submitBtn.textContent = "Sending..."; }
        fetch(FORM_ENDPOINT, { method: "POST", body: body, headers: { Accept: "application/json" } })
            .then(function (r) { return r.ok; })
            .then(function (ok) { modalDone(modalEl, ok ? sentScreen(sentMsg) : mailtoScreen(mailto, heading), close); })
            .catch(function () { modalDone(modalEl, mailtoScreen(mailto, heading), close); });
    }

    /* ---- "Contact us" modal ---- injected once, shared by every header button. */
    var contactTriggers = document.querySelectorAll(".js-contact");
    if (contactTriggers.length) {
        var overlay = document.createElement("div");
        overlay.className = "modal-overlay";
        overlay.hidden = true;
        overlay.innerHTML =
            '<div class="modal" role="dialog" aria-modal="true" aria-labelledby="contactTitle">'
          + '<button class="modal-x" type="button" aria-label="Close">'
          + '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg></button>'
          + '<h3 id="contactTitle">Contact us</h3>'
          + '<p class="muted">Tell us what you need and we will get back to you by email.</p>'
          + '<form id="contactForm" novalidate>'
          + '<div class="frow"><label>Your name<input type="text" name="name" placeholder="Jane Doe"></label>'
          + '<label>Topic<select name="topic" required><option value="" disabled selected>Select an option</option><option value="Enterprise Support">Enterprise Support</option><option value="Feature Development">Feature Development</option><option value="Engineering Collaboration">Engineering Collaboration</option><option value="Unsure / exploratory">Unsure / exploratory</option></select></label></div>'
          + '<label>Your email<input type="email" name="email" placeholder="you@company.com" required></label>'
          + '<label>How can we help?<textarea name="notes" rows="4" placeholder="Tell us about your use case"></textarea></label>'
          + '<input type="text" name="botcheck" tabindex="-1" autocomplete="off" aria-hidden="true" style="position:absolute;left:-9999px">'
          + '<button type="submit" class="btn btn-primary btn-pill">Send message</button>'
          + '</form></div>';
        document.body.appendChild(overlay);

        var modal = overlay.querySelector(".modal");
        var form = overlay.querySelector("#contactForm");

        function openModal(e) {
            if (e) e.preventDefault();
            overlay.hidden = false;
            document.body.style.overflow = "hidden";
        }
        function closeModal() { overlay.hidden = true; document.body.style.overflow = ""; }

        contactTriggers.forEach(function (b) { b.addEventListener("click", openModal); });
        overlay.querySelector(".modal-x").addEventListener("click", closeModal);
        overlay.addEventListener("click", function (e) { if (e.target === overlay) closeModal(); });
        document.addEventListener("keydown", function (e) { if (e.key === "Escape" && !overlay.hidden) closeModal(); });

        form.addEventListener("submit", function (e) {
            e.preventDefault();
            if (!form.email.value || !form.topic.value) {
                if (form.reportValidity) form.reportValidity();
                return;
            }
            var subject = "Duckle: " + form.topic.value;
            var mbody = "Hi Sourav,%0D%0A%0D%0A"
                + (form.name.value.trim() ? "Name: " + encodeURIComponent(form.name.value.trim()) + "%0D%0A" : "")
                + "Email: " + encodeURIComponent(form.email.value)
                + "%0D%0ATopic: " + encodeURIComponent(form.topic.value)
                + (form.notes.value.trim() ? "%0D%0A%0D%0A" + encodeURIComponent(form.notes.value.trim()) : "");
            var mailto = "mailto:" + HOST + "?subject=" + encodeURIComponent(subject) + "&body=" + mbody;
            deliverForm(modal, form.querySelector("button[type=submit]"), {
                subject: subject,
                from_name: form.name.value.trim() || form.email.value,
                name: form.name.value.trim(),
                email: form.email.value,
                topic: form.topic.value,
                message: form.notes.value.trim(),
                botcheck: form.botcheck && form.botcheck.value
            }, mailto, "Thanks for reaching out",
               "We have your details and will get back to you by email.", closeModal);
        });
    }

    /* ---- "Request a connector" modal ----
       Static site, no backend: a short form that opens the visitor's mail
       client with a prefilled request to the maintainer, who hand-builds the
       connector. Injected once, shared by every .js-connector trigger. */
    var connTriggers = document.querySelectorAll(".js-connector");
    if (connTriggers.length) {
        var cOverlay = document.createElement("div");
        cOverlay.className = "modal-overlay";
        cOverlay.hidden = true;
        cOverlay.innerHTML =
            '<div class="modal" role="dialog" aria-modal="true" aria-labelledby="connTitle">'
          + '<button class="modal-x" type="button" aria-label="Close">'
          + '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg></button>'
          + '<h3 id="connTitle">Request a connector</h3>'
          + '<p class="muted">Tell us the system you need. We build connectors by hand and will follow up by email.</p>'
          + '<form id="connForm" novalidate>'
          + '<div class="frow"><label>Connector<input type="text" name="conn" placeholder="e.g. NetSuite, IBM DB2" required></label>'
          + '<label>Direction<select name="dir"><option value="Source">Source (read from)</option><option value="Destination">Destination (write to)</option><option value="Both">Both</option></select></label></div>'
          + '<label>Your email<input type="email" name="email" placeholder="you@company.com" required></label>'
          + '<label>What do you need it for?<textarea name="notes" rows="3" placeholder="Auth method, API docs link, volume, and how you would use it"></textarea></label>'
          + '<input type="text" name="botcheck" tabindex="-1" autocomplete="off" aria-hidden="true" style="position:absolute;left:-9999px">'
          + '<button type="submit" class="btn btn-primary btn-pill">Send request</button>'
          + '</form></div>';
        document.body.appendChild(cOverlay);

        var cModal = cOverlay.querySelector(".modal");
        var cForm = cOverlay.querySelector("#connForm");

        function connOpen(e) {
            if (e) e.preventDefault();
            cOverlay.hidden = false;
            document.body.style.overflow = "hidden";
        }
        function connClose() { cOverlay.hidden = true; document.body.style.overflow = ""; }

        connTriggers.forEach(function (b) { b.addEventListener("click", connOpen); });
        cOverlay.querySelector(".modal-x").addEventListener("click", connClose);
        cOverlay.addEventListener("click", function (e) { if (e.target === cOverlay) connClose(); });
        document.addEventListener("keydown", function (e) { if (e.key === "Escape" && !cOverlay.hidden) connClose(); });

        cForm.addEventListener("submit", function (e) {
            e.preventDefault();
            if (!cForm.conn.value.trim() || !cForm.email.value) {
                if (cForm.reportValidity) cForm.reportValidity();
                return;
            }
            var subject = "Duckle connector request: " + cForm.conn.value.trim();
            var mbody = "Hi Sourav,%0D%0A%0D%0AI would like to request a Duckle connector."
                + "%0D%0A%0D%0AConnector: " + encodeURIComponent(cForm.conn.value.trim())
                + "%0D%0ADirection: " + encodeURIComponent(cForm.dir.value)
                + "%0D%0AEmail: " + encodeURIComponent(cForm.email.value)
                + (cForm.notes.value.trim() ? "%0D%0A%0D%0A" + encodeURIComponent(cForm.notes.value.trim()) : "");
            var cMailto = "mailto:" + HOST + "?subject=" + encodeURIComponent(subject) + "&body=" + mbody;
            deliverForm(cModal, cForm.querySelector("button[type=submit]"), {
                subject: subject,
                from_name: cForm.email.value,
                connector: cForm.conn.value.trim(),
                direction: cForm.dir.value,
                email: cForm.email.value,
                message: cForm.notes.value.trim(),
                botcheck: cForm.botcheck && cForm.botcheck.value
            }, cMailto, "Request ready to send",
               "We have your request and will get back to you by email.", connClose);
        });
    }

    /* ---- Discord widget: bottom-right floating button + dismissible invite popup ---- */
    (function () {
        var DISCORD = "https://discord.gg/rUeAStJbWb";
        var ICON = '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M20.317 4.369a19.79 19.79 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.249a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.036A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994a.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.01c.12.098.246.197.373.291a.077.077 0 0 1-.006.127 12.3 12.3 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03zM8.02 15.331c-1.182 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.956 2.418-2.157 2.418zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.418-2.157 2.418z"/></svg>';
        var wrap = document.createElement("div");
        wrap.className = "discord-widget";
        var dismissed = false;
        try { dismissed = localStorage.getItem("duckle-discord") === "1"; } catch (e) {}
        var pop = dismissed ? "" :
            '<div class="discord-pop" id="discordPop">'
          + '<button class="discord-pop-x" id="discordPopX" type="button" aria-label="Close">'
          + '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg></button>'
          + '<strong>Join us on Discord</strong>'
          + '<p>Bugs, support, help or ideas - come build with us.</p>'
          + '<a class="discord-pop-cta" href="' + DISCORD + '" target="_blank" rel="noopener">Open Discord</a>'
          + '</div>';
        wrap.innerHTML = pop
          + '<a class="discord-fab" href="' + DISCORD + '" target="_blank" rel="noopener" aria-label="Join Duckle on Discord">' + ICON + '</a>';
        document.body.appendChild(wrap);
        var dx = document.getElementById("discordPopX");
        if (dx) dx.addEventListener("click", function (e) {
            e.preventDefault();
            var p = document.getElementById("discordPop");
            if (p) p.remove();
            try { localStorage.setItem("duckle-discord", "1"); } catch (e) {}
        });
    })();
})();

// Copy-to-clipboard for the agent onboarding prompt. Reads the rendered text
// rather than a duplicated data- attribute, so the button can never drift out
// of sync with what the visitor is looking at.
(function () {
    var btns = document.querySelectorAll('[data-copy-target]');
    if (!btns.length) return;
    btns.forEach(function (btn) {
        btn.addEventListener('click', function () {
            var src = document.getElementById(btn.getAttribute('data-copy-target'));
            if (!src) return;
            var text = (src.innerText || src.textContent || '').trim();
            var label = btn.querySelector('.prompt-copy-label');
            var done = function (ok) {
                if (!label) return;
                var was = label.textContent;
                label.textContent = ok ? 'Copied' : 'Press Ctrl+C';
                btn.classList.toggle('copied', ok);
                setTimeout(function () {
                    label.textContent = was;
                    btn.classList.remove('copied');
                }, 1800);
            };
            if (navigator.clipboard && navigator.clipboard.writeText) {
                navigator.clipboard.writeText(text).then(function () { done(true); }, function () { done(false); });
                return;
            }
            // No async clipboard (older browser, or a non-secure origin):
            // select the text so the visitor can still copy it themselves.
            try {
                var r = document.createRange();
                r.selectNodeContents(src);
                var sel = window.getSelection();
                sel.removeAllRanges();
                sel.addRange(r);
                done(document.execCommand('copy'));
            } catch (e) { done(false); }
        });
    });
})();
