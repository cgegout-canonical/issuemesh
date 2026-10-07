async function runSearch() {
    const q = document.getElementById('query-input').value.trim();
    if (!q) return;

    const progressSec = document.getElementById('search-progress');
    const progressLog = document.getElementById('progress-log');
    const resultsList = document.getElementById('results-list');
    const searchBtn = document.getElementById('search-btn');

    searchBtn.disabled = true;
    progressSec.style.display = 'block';
    progressLog.innerHTML = '<div>&#9673; Searching local memory and trackers...</div>';
    resultsList.innerHTML = '';

    try {
        const resp = await fetch('/api/search/local?q=' + encodeURIComponent(q));
        const data = await resp.json();

        progressLog.innerHTML += '<div>&#10003; Retrieved ' + data.length + ' matching objects</div>';

        if (data.length === 0) {
            resultsList.innerHTML = '<div class="card"><p>No local matches found. (Remote and deep search pipelines execute via deep search jobs in Stages 7-10).</p></div>';
        } else {
            let html = '';
            for (const item of data) {
                const ev = item.evidence;
                const badgeClass = 'badge-' + ev.ref.source;
                html += `
                    <div class="card">
                        <div style="margin-bottom: 0.5rem;">
                            <span class="badge ${badgeClass}">${ev.ref.source}</span>
                            <span style="font-weight: 600; color: var(--color-muted-text);">${ev.ref.id}</span>
                            <span style="float: right; font-size: 0.85rem; color: var(--color-muted-text);">${ev.state}</span>
                        </div>
                        <div class="card-title"><a href="${ev.url}" target="_blank" style="color: var(--color-dark-text); text-decoration: none;">${escapeHtml(ev.title)}</a></div>
                        <p style="color: var(--color-muted-text); font-size: 0.95rem;">${item.snippet || ev.snippet}</p>
                    </div>
                `;
            }
            resultsList.innerHTML = html;
        }
    } catch (e) {
        progressLog.innerHTML += '<div style="color: var(--color-red);">&#10007; Search error: ' + e + '</div>';
    } finally {
        searchBtn.disabled = false;
    }
}

function escapeHtml(s) {
    return (s || '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
