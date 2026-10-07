let currentJobId = null;

async function runSearch() {
    const q = document.getElementById('query-input').value.trim();
    if (!q) return;

    const progressSec = document.getElementById('search-progress');
    const progressLog = document.getElementById('progress-log');
    const resultsArea = document.getElementById('results-area');
    const searchBtn = document.getElementById('search-btn');

    searchBtn.disabled = true;
    progressSec.style.display = 'block';
    progressLog.innerHTML = '<div>&#9673; Submitting search job...</div>';
    resultsArea.innerHTML = '';

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
        currentJobId = data.job_id;
        progressLog.innerHTML += `<div>&#10003; Job created: <code>${currentJobId}</code>. Listening for live events...</div>`;

        // Connect SSE stream
        const eventSource = new EventSource(`/api/search/${currentJobId}/events`);

        eventSource.onmessage = (event) => {
            try {
                const ev = JSON.parse(event.data);
                handleSearchEvent(ev, progressLog);

                if (ev.type === 'Complete' || ev.type === 'Error') {
                    eventSource.close();
                    searchBtn.disabled = false;
                    loadStructuredResult(currentJobId);
                }
            } catch (err) {
                console.error("Error parsing SSE event:", err);
            }
        };

        eventSource.onerror = (e) => {
            console.warn("SSE connection closed or completed:", e);
            eventSource.close();
            searchBtn.disabled = false;
            loadStructuredResult(currentJobId);
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

async function loadStructuredResult(jobId) {
    const resultsArea = document.getElementById('results-area');
    try {
        const resp = await fetch(`/api/search/${encodeURIComponent(jobId)}/result`);
        if (!resp.ok) {
            throw new Error(`Failed to load result (HTTP ${resp.status})`);
        }
        const data = await resp.json();
        renderStructuredResult(jobId, data);
    } catch (e) {
        resultsArea.innerHTML = `<div class="card"><p style="color: var(--color-red);">Error loading results: ${escapeHtml(String(e))}</p></div>`;
    }
}

function renderStructuredResult(jobId, result) {
    const container = document.getElementById('results-area');
    let html = '';

    // 1. Synthesized Summary / Answer (SPEC.md §37)
    if (result.answer) {
        html += `
            <div class="card" style="border-left: 4px solid var(--color-orange);">
                <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem;">
                    <div class="card-title" style="margin: 0;">Synthesized Engineering Analysis</div>
                    <a href="/search/${encodeURIComponent(jobId)}/run" class="btn btn-secondary" style="font-size: 0.8rem; padding: 0.3rem 0.6rem; text-decoration: none;">Inspect Run</a>
                </div>
                <div style="white-space: pre-wrap; line-height: 1.6; color: var(--color-dark-text); font-size: 1.05rem;">
                    ${escapeHtml(result.answer)}
                </div>
            </div>
        `;
    }

    // 2. Consistency Warnings / Discrepancies (SPEC.md §38)
    if (result.findings && result.findings.length > 0) {
        html += `
            <div class="card" style="border-left: 4px solid var(--color-aubergine);">
                <div class="card-title" style="color: var(--color-aubergine);">Consistency & Workflow Findings (${result.findings.length})</div>
                <div style="display: flex; flex-direction: column; gap: 0.75rem; margin-top: 0.5rem;">
        `;
        for (const f of result.findings) {
            const sevColor = f.severity === 'Warning' ? 'var(--color-orange)' : '#4A5568';
            html += `
                <div style="padding: 0.75rem; background: var(--color-warm-grey); border-radius: 2px;">
                    <div style="display: flex; gap: 0.5rem; align-items: center; margin-bottom: 0.25rem;">
                        <span style="font-weight: 600; font-size: 0.85rem; color: ${sevColor}; text-transform: uppercase;">${f.finding_type}</span>
                        <span style="color: var(--color-muted-text); font-size: 0.85rem;">·</span>
                        <span style="font-size: 0.85rem; color: var(--color-muted-text);">${f.severity}</span>
                    </div>
                    <div style="font-size: 0.95rem; color: var(--color-dark-text);">${escapeHtml(f.description)}</div>
                </div>
            `;
        }
        html += `</div></div>`;
    }

    // 3. Key Findings & Ranked Evidence Cards (SPEC.md §37, §38)
    const ranked = result.ranked_results || [];
    html += `
        <div style="margin-top: 1.5rem; margin-bottom: 1rem; display: flex; justify-content: space-between; align-items: center;">
            <h2 style="font-size: 1.3rem; font-weight: 400;">Ranked Engineering Evidence (${ranked.length})</h2>
            <div style="font-size: 0.85rem; color: var(--color-muted-text);">
                Coverage: ${result.stats?.trackers_searched ?? 0} trackers · ${result.stats?.objects_analysed ?? 0} objects evaluated
            </div>
        </div>
    `;

    if (ranked.length === 0) {
        html += `<div class="card"><p>No relevant evidence matched the search query.</p></div>`;
    } else {
        for (let i = 0; i < ranked.length; i++) {
            const item = ranked[i];
            const ev = item.evidence;
            const badgeClass = 'badge-' + ev.ref.source;
            const cardId = `evidence-${i}`;

            html += `
                <div class="card" id="${cardId}">
                    <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem;">
                        <div>
                            <span class="badge ${badgeClass}">${ev.ref.source}</span>
                            <span style="font-weight: 600; font-family: var(--font-mono); color: var(--color-muted-text);">${ev.ref.id}</span>
                            <span style="font-size: 0.8rem; background: var(--color-warm-grey); padding: 0.1rem 0.4rem; border-radius: 2px; margin-left: 0.5rem;">Score: ${item.score.toFixed(3)}</span>
                        </div>
                        <span style="font-size: 0.85rem; font-weight: 500; color: var(--color-muted-text);">${escapeHtml(ev.state || '')}</span>
                    </div>

                    <div class="card-title">
                        <a href="${ev.url}" target="_blank" style="color: var(--color-dark-text); text-decoration: none;">
                            ${escapeHtml(ev.title)}
                        </a>
                    </div>

                    <p style="color: var(--color-muted-text); font-size: 0.95rem; margin-bottom: 0.75rem;">
                        ${escapeHtml(ev.snippet || '')}
                    </p>

                    ${ev.body ? `
                        <details style="margin-top: 0.5rem; font-size: 0.9rem;">
                            <summary style="cursor: pointer; color: var(--color-orange); font-weight: 500;">View Full Body Text</summary>
                            <pre style="margin-top: 0.5rem; padding: 0.75rem; background: var(--color-warm-grey); border-radius: 2px; white-space: pre-wrap; font-family: inherit; font-size: 0.9rem;">${escapeHtml(ev.body)}</pre>
                        </details>
                    ` : ''}

                    <div style="margin-top: 0.75rem; display: flex; gap: 1rem; font-size: 0.8rem; color: var(--color-muted-text);">
                        ${ev.author ? `<span>Author: <strong>${escapeHtml(ev.author)}</strong></span>` : ''}
                        ${ev.container ? `<span>Container: <strong>${escapeHtml(ev.container)}</strong></span>` : ''}
                        ${ev.updated_at ? `<span>Updated: <strong>${formatTime(ev.updated_at)}</strong></span>` : ''}
                    </div>
                </div>
            `;
        }
    }

    // 4. Relationships Followed / Graph Traversal (SPEC.md §38)
    if (result.relations_followed && result.relations_followed.length > 0) {
        html += `
            <div class="card" style="margin-top: 1.5rem;">
                <div class="card-title">Cross-Tracker Relationships Traversed (${result.relations_followed.length})</div>
                <div style="font-family: var(--font-mono); font-size: 0.85rem; display: flex; flex-direction: column; gap: 0.4rem; margin-top: 0.5rem;">
        `;
        for (const rel of result.relations_followed) {
            html += `
                <div style="padding: 0.4rem 0.6rem; background: var(--color-warm-grey); border-radius: 2px;">
                    <strong>${rel.source_ref.id}</strong> 
                    <span style="color: var(--color-orange);">—[${rel.relation_type}]&rarr;</span> 
                    <strong>${rel.target_ref.id}</strong>
                    ${rel.target_url ? `(<a href="${rel.target_url}" target="_blank" style="color: var(--color-muted-text); text-decoration: underline;">link</a>)` : ''}
                </div>
            `;
        }
        html += `</div></div>`;
    }

    container.innerHTML = html;
}

function formatTime(iso) {
    try {
        const d = new Date(iso);
        return d.toLocaleDateString();
    } catch {
        return iso;
    }
}

function escapeHtml(s) {
    return (s || '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
