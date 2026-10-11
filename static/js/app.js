let currentJobId = null;

function countWords(str) {
    if (!str || !str.trim()) return 0;
    return str.trim().split(/\s+/).length;
}

function clampWords(str, maxWords = 250) {
    if (!str || !str.trim()) return '';
    const words = str.trim().split(/\s+/);
    if (words.length <= maxWords) {
        return str.trim();
    }
    return words.slice(0, maxWords).join(' ') + '...';
}

function generateSummary(result) {
    if (result.answer && result.answer.trim().length > 0) {
        return clampWords(result.answer.trim(), 250);
    }

    const ranked = result.ranked_results || [];
    const findings = result.findings || [];
    const stats = result.stats || {};
    const trackersCount = stats.trackers_searched ?? 0;
    const objectsCount = stats.objects_analysed ?? 0;
    const duration = stats.duration_ms ?? 0;

    let parts = [];
    if (ranked.length === 0) {
        parts.push("I could not find sufficient evidence in Jira, GitHub or Launchpad to answer this reliably.");
        parts.push(`Search completed in ${duration}ms across ${trackersCount} trackers (${objectsCount} objects evaluated). No matching evidence items were found for this query.`);
    } else {
        parts.push(`Search completed in ${duration}ms across ${trackersCount} trackers, evaluating ${objectsCount} objects and identifying ${ranked.length} relevant engineering evidence items.`);
        if (findings.length > 0) {
            parts.push(`Detected ${findings.length} cross-tracker consistency and workflow findings.`);
        }
        const topSummary = ranked.slice(0, 3).map(r => `• [${r.evidence.ref.source.toUpperCase()}] ${r.evidence.title} (${r.evidence.ref.id})`).join('\n');
        parts.push(`Top ranked matches:\n${topSummary}`);
    }
    return clampWords(parts.join('\n\n'), 250);
}

function copySummary() {
    const box = document.getElementById('summary-textbox');
    if (!box) return;
    navigator.clipboard.writeText(box.value).then(() => {
        const btn = document.getElementById('copy-summary-btn');
        if (btn) {
            const orig = btn.innerText;
            btn.innerText = 'Copied!';
            setTimeout(() => { btn.innerText = orig; }, 1500);
        }
    }).catch(err => console.warn('Clipboard write failed:', err));
}

async function runSearch() {
    const q = document.getElementById('query-input').value.trim();
    if (!q) return;

    const progressBar = document.getElementById('search-progress-bar');
    const progressSec = document.getElementById('search-progress');
    const progressLog = document.getElementById('progress-log');
    const progressBox = document.getElementById('progress-textbox');
    const statusBadge = document.getElementById('search-progress-status');
    const resultsArea = document.getElementById('results-area');
    const searchBtn = document.getElementById('search-btn');

    searchBtn.disabled = true;
    if (progressBar) progressBar.classList.add('active');
    if (progressSec) progressSec.style.display = 'block';
    if (statusBadge) statusBadge.innerHTML = '<span class="dot dot-searching"></span> Live';
    if (progressBox) progressBox.value = '◉ Submitting search job...';
    if (progressLog) progressLog.innerHTML = '<div>&#9673; Submitting search job...</div>';
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
        if (progressBox) {
            progressBox.value += `\n✓ Job created: ${currentJobId}. Listening for live events...`;
            progressBox.scrollTop = progressBox.scrollHeight;
        }
        if (progressLog) {
            progressLog.innerHTML += `<div>&#10003; Job created: <code>${currentJobId}</code>. Listening for live events...</div>`;
        }

        // Connect SSE stream
        const eventSource = new EventSource(`/api/search/${currentJobId}/events`);

        eventSource.onmessage = (event) => {
            try {
                const ev = JSON.parse(event.data);
                handleSearchEvent(ev, progressLog, progressBox);

                if (ev.type === 'Complete' || ev.type === 'Error') {
                    eventSource.close();
                    if (progressBar) progressBar.classList.remove('active');
                    if (statusBadge) statusBadge.innerHTML = '<span class="dot dot-idle"></span> Done';
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
            if (progressBar) progressBar.classList.remove('active');
            if (statusBadge) statusBadge.innerHTML = '<span class="dot dot-idle"></span> Done';
            searchBtn.disabled = false;
            loadStructuredResult(currentJobId);
        };
    } catch (e) {
        if (progressBar) progressBar.classList.remove('active');
        if (statusBadge) statusBadge.innerHTML = '<span class="dot dot-failed"></span> Error';
        if (progressBox) {
            progressBox.value += `\n✗ Search error: ${e.message || String(e)}`;
            progressBox.scrollTop = progressBox.scrollHeight;
        }
        if (progressLog) {
            progressLog.innerHTML += `<div style="color: var(--color-red);">&#10007; Search error: ${escapeHtml(e.message || String(e))}</div>`;
        }
        searchBtn.disabled = false;
    }
}

function handleSearchEvent(ev, logElem, boxElem) {
    let msg = '';
    let plainText = '';

    switch (ev.type) {
        case 'PlanningStarted':
            msg = '&#9673; Planning search strategy...';
            plainText = '◉ Planning search strategy...';
            break;
        case 'PlanningCompleted':
            msg = '&#10003; Planning completed';
            plainText = '✓ Planning completed';
            break;
        case 'LocalSearchStarted':
            msg = '&#9673; Searching local datastore (FTS5)...';
            plainText = '◉ Searching local datastore (FTS5)...';
            break;
        case 'LocalSearchCompleted':
            msg = `&#10003; Local search found ${ev.data?.objects ?? 0} objects`;
            plainText = `✓ Local search found ${ev.data?.objects ?? 0} objects`;
            break;
        case 'SourceSearchStarted':
            msg = `&#9673; Querying remote source: <strong>${ev.data?.source ?? ''}</strong>...`;
            plainText = `◉ Querying remote source: ${ev.data?.source ?? ''}...`;
            break;
        case 'SourceSearchCompleted':
            msg = `&#10003; Retrieved ${ev.data?.objects ?? 0} objects from ${ev.data?.source ?? ''}`;
            plainText = `✓ Retrieved ${ev.data?.objects ?? 0} objects from ${ev.data?.source ?? ''}`;
            break;
        case 'DeepReadStarted':
            msg = `&#9673; Deep reading object ${ev.data?.object?.id ?? ''}...`;
            plainText = `◉ Deep reading object ${ev.data?.object?.id ?? ''}...`;
            break;
        case 'DeepReadCompleted':
            msg = `&#10003; Finished deep read for ${ev.data?.object?.id ?? ''}`;
            plainText = `✓ Finished deep read for ${ev.data?.object?.id ?? ''}`;
            break;
        case 'GraphExpansion':
            msg = `&#10003; Graph frontier expanded (depth ${ev.data?.depth ?? 0}, nodes: ${ev.data?.nodes ?? 0})`;
            plainText = `✓ Graph frontier expanded (depth ${ev.data?.depth ?? 0}, nodes: ${ev.data?.nodes ?? 0})`;
            break;
        case 'ConsistencyCheckStarted':
            msg = '&#9673; Performing cross-tracker consistency analysis...';
            plainText = '◉ Performing cross-tracker consistency analysis...';
            break;
        case 'ConsistencyCheckCompleted':
            msg = `&#10003; Consistency checks complete (${ev.data?.findings ?? 0} findings)`;
            plainText = `✓ Consistency checks complete (${ev.data?.findings ?? 0} findings)`;
            break;
        case 'SynthesisStarted':
            msg = '&#9673; Synthesizing evidence...';
            plainText = '◉ Synthesizing evidence...';
            break;
        case 'Complete': {
            const stats = ev.data || {};
            msg = `<strong>&#10003; Search complete!</strong> Analyzed ${stats.objects_analysed ?? 0} objects, ${stats.deep_reads ?? 0} deep reads, ${stats.relations_followed ?? 0} relations followed across ${stats.trackers_searched ?? 0} trackers (${stats.duration_ms ?? 0}ms)`;
            plainText = `✓ Search complete! Analyzed ${stats.objects_analysed ?? 0} objects across ${stats.trackers_searched ?? 0} trackers (${stats.duration_ms ?? 0}ms)`;
            break;
        }
        case 'Warning':
            msg = `<span style="color: var(--color-orange);">&#9888; Warning: ${escapeHtml(ev.data ?? '')}</span>`;
            plainText = `⚠ Warning: ${ev.data ?? ''}`;
            break;
        case 'Error':
            msg = `<span style="color: var(--color-red);">&#10007; Error: ${escapeHtml(ev.data ?? '')}</span>`;
            plainText = `✗ Error: ${ev.data ?? ''}`;
            break;
        default:
            msg = `Event: ${escapeHtml(JSON.stringify(ev))}`;
            plainText = `Event: ${JSON.stringify(ev)}`;
    }

    if (boxElem && plainText) {
        boxElem.value += '\n' + plainText;
        boxElem.scrollTop = boxElem.scrollHeight;
    }

    if (logElem && msg) {
        const row = document.createElement('div');
        row.innerHTML = msg;
        logElem.appendChild(row);
        logElem.scrollTop = logElem.scrollHeight;
    }
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

    // 1. Textbox with summary of results (guaranteed <= 250 words)
    const summaryText = generateSummary(result);
    const wordCount = countWords(summaryText);
    const summaryLines = summaryText.split('\n').length;
    const calculatedRows = Math.max(4, Math.min(10, summaryLines + 2));

    html += `
        <div class="summary-card">
            <div class="summary-header">
                <div class="summary-title-wrapper">
                    <h2 class="summary-title">Summary of Results</h2>
                    <span class="summary-word-badge" id="summary-word-counter">${wordCount} / 250 words</span>
                </div>
                <div class="summary-actions">
                    <button type="button" id="copy-summary-btn" class="btn btn-secondary" style="font-size: 0.8rem; padding: 0.3rem 0.6rem;" onclick="copySummary()">Copy Summary</button>
                    <a href="/search/${encodeURIComponent(jobId)}/run" class="btn btn-secondary" style="font-size: 0.8rem; padding: 0.3rem 0.6rem; text-decoration: none;">Inspect Run</a>
                </div>
            </div>
            <textarea id="summary-textbox" class="summary-textbox" readonly rows="${calculatedRows}">${escapeHtml(summaryText)}</textarea>
        </div>
    `;

    // 2. Evidence-Based Confidence (Objective Metric)
    if (result.confidence) {
        const conf = result.confidence;
        let badgeColor = 'var(--color-green)';
        if (conf.level === 'Medium') badgeColor = 'var(--color-orange)';
        if (conf.level === 'Low' || conf.level === 'Uncertain') badgeColor = 'var(--color-red)';

        html += `
            <div class="card" style="border-left: 4px solid ${badgeColor};">
                <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem;">
                    <div class="card-title" style="margin: 0;">Evidence-Based Confidence: <span style="color: ${badgeColor};">${escapeHtml(conf.level)} (${Math.round(conf.score * 100)}%)</span></div>
                    <span style="font-size: 0.8rem; color: var(--color-muted-text);">Objective Multi-Source Signal</span>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem; font-size: 0.85rem; color: var(--color-dark-text); margin-top: 0.5rem;">
                    ${(conf.factors || []).map(f => `<div>&bull; ${escapeHtml(f)}</div>`).join('')}
                </div>
            </div>
        `;
    }

    // 3. Consistency Warnings / Discrepancies (SPEC.md §38)
    if (result.findings && result.findings.length > 0) {
        html += `
            <div class="card" style="border-left: 4px solid var(--color-slate);">
                <div class="card-title" style="color: var(--color-slate);">Consistency & Workflow Findings (${result.findings.length})</div>
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
                    <div style="font-size: 0.95rem; color: var(--color-dark-text);">${escapeHtml(f.explanation || f.description || '')}</div>
                </div>
            `;
        }
        html += `</div></div>`;
    }

    // 4. Ranked Evidence Cards listed below (SPEC.md §37, §38)
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
            const objKey = `${ev.ref.source}:${ev.ref.type}:${ev.ref.id}`;

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

                    <div style="margin-top: 0.75rem; display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 0.5rem;">
                        <div style="display: flex; gap: 1rem; font-size: 0.8rem; color: var(--color-muted-text);">
                            ${ev.author ? `<span>Author: <strong>${escapeHtml(ev.author)}</strong></span>` : ''}
                            ${ev.container ? `<span>Container: <strong>${escapeHtml(ev.container)}</strong></span>` : ''}
                            ${ev.updated_at ? `<span>Updated: <strong>${formatTime(ev.updated_at)}</strong></span>` : ''}
                        </div>
                        <div style="display: flex; gap: 0.4rem;">
                            <a href="/intelligence?tab=impact&key=${encodeURIComponent(objKey)}" class="btn btn-secondary" style="font-size: 0.75rem; padding: 0.2rem 0.5rem; text-decoration: none;">Impact</a>
                            <a href="/intelligence?tab=timeline&key=${encodeURIComponent(objKey)}" class="btn btn-secondary" style="font-size: 0.75rem; padding: 0.2rem 0.5rem; text-decoration: none;">Timeline</a>
                            <a href="/intelligence?tab=watchlists&key=${encodeURIComponent(objKey)}" class="btn btn-secondary" style="font-size: 0.75rem; padding: 0.2rem 0.5rem; text-decoration: none;">Diffs</a>
                        </div>
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
