async function runSearch() {
    const q = document.getElementById('query-input').value.trim();
    if (!q) return;

    const progressSec = document.getElementById('search-progress');
    const progressLog = document.getElementById('progress-log');
    const resultsList = document.getElementById('results-list');
    const searchBtn = document.getElementById('search-btn');

    searchBtn.disabled = true;
    progressSec.style.display = 'block';
    progressLog.innerHTML = '<div>&#9673; Submitting search job...</div>';
    resultsList.innerHTML = '';

    try {
        const resp = await fetch('/api/search', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ query: q })
        });

        if (!resp.ok) {
            const err = await resp.json().catch(() => ({ error: resp.statusText }));
            throw new Error(err.error || 'Failed to start search');
        }

        const data = await resp.json();
        const jobId = data.job_id;
        progressLog.innerHTML += `<div>&#10003; Job created: <code>${jobId}</code>. Listening for live events...</div>`;

        // Connect SSE stream
        const eventSource = new EventSource(`/api/search/${jobId}/events`);

        eventSource.onmessage = (event) => {
            try {
                const ev = JSON.parse(event.data);
                handleSearchEvent(ev, progressLog);

                if (ev.type === 'Complete' || ev.type === 'Error') {
                    eventSource.close();
                    searchBtn.disabled = false;
                    loadLocalResults(q);
                }
            } catch (err) {
                console.error("Error parsing SSE event:", err);
            }
        };

        eventSource.onerror = (e) => {
            console.warn("SSE connection closed or errored:", e);
            eventSource.close();
            searchBtn.disabled = false;
            loadLocalResults(q);
        };
    } catch (e) {
        progressLog.innerHTML += `<div style="color: var(--color-red);">&#10007; Search error: ${escapeHtml(e.message || String(e))}</div>`;
        searchBtn.disabled = false;
    }
}

function handleSearchEvent(ev, logElem) {
    let msg = '';
    switch (ev.type) {
        case 'PlanningStarted':
            msg = '&#9673; Planning search strategy...';
            break;
        case 'PlanningCompleted':
            msg = '&#10003; Planning completed';
            break;
        case 'LocalSearchStarted':
            msg = '&#9673; Searching local datastore (FTS5)...';
            break;
        case 'LocalSearchCompleted':
            msg = `&#10003; Local search found ${ev.data?.objects ?? 0} objects`;
            break;
        case 'SourceSearchStarted':
            msg = `&#9673; Querying remote source: <strong>${ev.data?.source ?? ''}</strong>...`;
            break;
        case 'SourceSearchCompleted':
            msg = `&#10003; Retrieved ${ev.data?.objects ?? 0} objects from ${ev.data?.source ?? ''}`;
            break;
        case 'DeepReadStarted':
            msg = `&#9673; Deep reading object ${ev.data?.object?.id ?? ''}...`;
            break;
        case 'DeepReadCompleted':
            msg = `&#10003; Finished deep read for ${ev.data?.object?.id ?? ''}`;
            break;
        case 'GraphExpansion':
            msg = `&#10003; Graph frontier expanded (depth ${ev.data?.depth ?? 0}, nodes: ${ev.data?.nodes ?? 0})`;
            break;
        case 'ConsistencyCheckStarted':
            msg = '&#9673; Performing cross-tracker consistency analysis...';
            break;
        case 'ConsistencyCheckCompleted':
            msg = `&#10003; Consistency checks complete (${ev.data?.findings ?? 0} findings)`;
            break;
        case 'SynthesisStarted':
            msg = '&#9673; Synthesizing evidence...';
            break;
        case 'Complete': {
            const stats = ev.data || {};
            msg = `<strong>&#10003; Search complete!</strong> Analyzed ${stats.objects_analysed ?? 0} objects, ${stats.deep_reads ?? 0} deep reads, ${stats.relations_followed ?? 0} relations followed across ${stats.trackers_searched ?? 0} trackers (${stats.duration_ms ?? 0}ms)`;
            break;
        }
        case 'Warning':
            msg = `<span style="color: var(--color-orange);">&#9888; Warning: ${escapeHtml(ev.data ?? '')}</span>`;
            break;
        case 'Error':
            msg = `<span style="color: var(--color-red);">&#10007; Error: ${escapeHtml(ev.data ?? '')}</span>`;
            break;
        default:
            msg = `Event: ${escapeHtml(JSON.stringify(ev))}`;
    }

    const row = document.createElement('div');
    row.innerHTML = msg;
    logElem.appendChild(row);
    logElem.scrollTop = logElem.scrollHeight;
}

async function loadLocalResults(q) {
    const resultsList = document.getElementById('results-list');
    try {
        const resp = await fetch('/api/search/local?q=' + encodeURIComponent(q));
        const data = await resp.json();

        if (data.length === 0) {
            resultsList.innerHTML = '<div class="card"><p>No matching objects found.</p></div>';
            return;
        }

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
    } catch (e) {
        resultsList.innerHTML = `<div class="card"><p style="color: var(--color-red);">Failed to render results: ${escapeHtml(String(e))}</p></div>`;
    }
}

function escapeHtml(s) {
    return (s || '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
