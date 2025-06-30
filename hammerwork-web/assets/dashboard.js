/**
 * Hammerwork Dashboard JavaScript
 * Handles WebSocket connections, API calls, and UI interactions
 */

class HammerworkDashboard {
    constructor() {
        this.websocket = null;
        this.reconnectAttempts = 0;
        this.maxReconnectAttempts = 5;
        this.reconnectInterval = 5000;
        this.charts = {};
        this.lastUpdate = null;
        this.refreshInterval = null;
        this.authCredentials = null; // Store auth credentials
        
        this.init();
    }

    async init() {
        console.log('Initializing Hammerwork Dashboard...');
        
        // Initialize UI event listeners
        this.initializeEventListeners();
        
        // Initialize charts
        this.initializeCharts();
        
        // Try to load initial data (will prompt for auth if needed)
        await this.loadInitialData();
        
        // Connect WebSocket for real-time updates
        this.connectWebSocket();
        
        // Set up periodic refresh as fallback
        this.startPeriodicRefresh();
        
        console.log('Dashboard initialized successfully');
    }

    async promptForCredentials() {
        return new Promise((resolve) => {
            const username = prompt('Username:');
            if (username === null) {
                resolve(null);
                return;
            }
            
            const password = prompt('Password:');
            if (password === null) {
                resolve(null);
                return;
            }
            
            // Create base64 encoded credentials
            const credentials = btoa(`${username}:${password}`);
            resolve(credentials);
        });
    }

    initializeEventListeners() {
        // Refresh button
        document.getElementById('refreshBtn').addEventListener('click', () => {
            this.refreshAllData();
        });

        // Add job button
        document.getElementById('addQueueBtn').addEventListener('click', () => {
            this.showAddJobModal();
        });

        // Queue search
        document.getElementById('queueSearch').addEventListener('input', (e) => {
            this.filterQueues(e.target.value);
        });

        // Job filters
        document.getElementById('jobStatusFilter').addEventListener('change', () => {
            this.loadJobs();
        });

        document.getElementById('jobQueueFilter').addEventListener('change', () => {
            this.loadJobs();
        });

        // Chart period selector
        document.getElementById('throughputPeriod').addEventListener('change', (e) => {
            this.updateThroughputChart(e.target.value);
        });

        // Modal close handlers
        document.querySelectorAll('.modal-close').forEach(closeBtn => {
            closeBtn.addEventListener('click', (e) => {
                this.hideModal(e.target.closest('.modal'));
            });
        });

        // Modal backdrop click
        document.querySelectorAll('.modal').forEach(modal => {
            modal.addEventListener('click', (e) => {
                if (e.target === modal) {
                    this.hideModal(modal);
                }
            });
        });

        // Add job form submission
        document.getElementById('submitJobBtn').addEventListener('click', () => {
            this.submitNewJob();
        });

        // Job action buttons (will be added dynamically)
        document.addEventListener('click', (e) => {
            if (e.target.classList.contains('retry-job-btn')) {
                this.retryJob(e.target.dataset.jobId);
            } else if (e.target.classList.contains('delete-job-btn')) {
                this.deleteJob(e.target.dataset.jobId);
            } else if (e.target.classList.contains('view-job-btn')) {
                this.showJobDetails(e.target.dataset.jobId);
            }
        });

        // Keyboard shortcuts
        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                this.hideAllModals();
            } else if (e.key === 'r' && (e.ctrlKey || e.metaKey)) {
                e.preventDefault();
                this.refreshAllData();
            }
        });
    }

    initializeCharts() {
        // Throughput chart
        const throughputCtx = document.getElementById('throughputChart').getContext('2d');
        this.charts.throughput = new Chart(throughputCtx, {
            type: 'line',
            data: {
                labels: [],
                datasets: [{
                    label: 'Jobs Processed',
                    data: [],
                    borderColor: '#2563eb',
                    backgroundColor: 'rgba(37, 99, 235, 0.1)',
                    fill: true,
                    tension: 0.4
                }]
            },
            options: {
                responsive: true,
                maintainAspectRatio: false,
                scales: {
                    y: {
                        beginAtZero: true,
                        ticks: {
                            precision: 0
                        }
                    }
                },
                plugins: {
                    legend: {
                        display: false
                    }
                }
            }
        });

        // Queue status chart (doughnut)
        const queueStatusCtx = document.getElementById('queueStatusChart').getContext('2d');
        this.charts.queueStatus = new Chart(queueStatusCtx, {
            type: 'doughnut',
            data: {
                labels: ['Pending', 'Running', 'Completed', 'Failed'],
                datasets: [{
                    data: [0, 0, 0, 0],
                    backgroundColor: [
                        '#f59e0b',  // warning - pending
                        '#2563eb',  // primary - running
                        '#10b981',  // success - completed
                        '#ef4444',  // danger - failed
                    ],
                    borderWidth: 0
                }]
            },
            options: {
                responsive: true,
                maintainAspectRatio: false,
                plugins: {
                    legend: {
                        position: 'bottom'
                    }
                }
            }
        });
    }

    async loadInitialData() {
        console.log('Loading initial data...');
        
        try {
            await Promise.all([
                this.loadSystemOverview(),
                this.loadQueues(),
                this.loadJobs(),
                this.updateThroughputChart('24h')
            ]);
            
            this.lastUpdate = new Date();
            console.log('Initial data loaded successfully');
        } catch (error) {
            console.error('Failed to load initial data:', error);
            this.showError('Failed to load dashboard data. Please check your connection.');
        }
    }

    async loadSystemOverview() {
        try {
            const response = await this.apiCall('/api/stats/overview');
            if (response.success) {
                this.updateOverviewCards(response.data);
            }
        } catch (error) {
            console.error('Failed to load system overview:', error);
        }
    }

    updateOverviewCards(data) {
        document.getElementById('totalJobs').textContent = this.formatNumber(data.total_jobs || 0);
        document.getElementById('pendingJobs').textContent = this.formatNumber(data.pending_jobs || 0);
        document.getElementById('runningJobs').textContent = this.formatNumber(data.running_jobs || 0);
        document.getElementById('errorRate').textContent = this.formatPercentage(data.error_rate || 0);
        document.getElementById('throughput').textContent = this.formatNumber(data.throughput || 0);
        document.getElementById('avgProcessing').textContent = this.formatDuration(data.avg_processing_time || 0);

        // Update queue status chart
        if (this.charts.queueStatus) {
            this.charts.queueStatus.data.datasets[0].data = [
                data.pending_jobs || 0,
                data.running_jobs || 0,
                data.completed_jobs || 0,
                data.failed_jobs || 0
            ];
            this.charts.queueStatus.update();
        }
    }

    async loadQueues() {
        try {
            const response = await this.apiCall('/api/queues');
            if (response.success) {
                this.updateQueuesTable(response.data.queues || []);
                this.updateQueueFilter(response.data.queues || []);
            }
        } catch (error) {
            console.error('Failed to load queues:', error);
        }
    }

    updateQueuesTable(queues) {
        const tbody = document.querySelector('#queuesTable tbody');
        
        if (queues.length === 0) {
            tbody.innerHTML = '<tr class="loading-row"><td colspan="8">No queues found</td></tr>';
            return;
        }

        tbody.innerHTML = queues.map(queue => `
            <tr>
                <td class="font-mono">${this.escapeHtml(queue.name)}</td>
                <td>${this.formatNumber(queue.pending_count || 0)}</td>
                <td>${this.formatNumber(queue.running_count || 0)}</td>
                <td>${this.formatNumber(queue.completed_count || 0)}</td>
                <td>${this.formatNumber(queue.failed_count || 0)}</td>
                <td>${this.formatNumber(queue.throughput || 0)}/min</td>
                <td>${this.formatPercentage(queue.error_rate || 0)}</td>
                <td>
                    <button class="btn btn-sm btn-secondary" onclick="dashboard.clearQueue('${queue.name}')">Clear</button>
                    <button class="btn btn-sm btn-danger" onclick="dashboard.pauseQueue('${queue.name}')">Pause</button>
                </td>
            </tr>
        `).join('');
    }

    updateQueueFilter(queues) {
        const select = document.getElementById('jobQueueFilter');
        const currentValue = select.value;
        
        select.innerHTML = '<option value="">All Queues</option>' +
            queues.map(queue => `<option value="${queue.name}">${this.escapeHtml(queue.name)}</option>`).join('');
        
        if (currentValue) {
            select.value = currentValue;
        }
    }

    async loadJobs() {
        try {
            const statusFilter = document.getElementById('jobStatusFilter').value;
            const queueFilter = document.getElementById('jobQueueFilter').value;
            
            let url = '/api/jobs?limit=50';
            if (statusFilter) url += `&status=${statusFilter}`;
            if (queueFilter) url += `&queue=${encodeURIComponent(queueFilter)}`;
            
            const response = await this.apiCall(url);
            if (response.success) {
                this.updateJobsTable(response.data.jobs || []);
            }
        } catch (error) {
            console.error('Failed to load jobs:', error);
        }
    }

    updateJobsTable(jobs) {
        const tbody = document.querySelector('#jobsTable tbody');
        
        if (jobs.length === 0) {
            tbody.innerHTML = '<tr class="loading-row"><td colspan="7">No jobs found</td></tr>';
            return;
        }

        tbody.innerHTML = jobs.map(job => `
            <tr>
                <td class="font-mono truncate" title="${job.id}">${job.id.substring(0, 8)}</td>
                <td class="truncate">${this.escapeHtml(job.queue_name)}</td>
                <td><span class="status-badge status-${job.status}">${job.status}</span></td>
                <td><span class="priority-badge priority-${job.priority}">${job.priority}</span></td>
                <td>${job.attempts || 0}</td>
                <td title="${job.created_at}">${this.formatRelativeTime(job.created_at)}</td>
                <td>
                    <button class="btn btn-sm btn-secondary view-job-btn" data-job-id="${job.id}">View</button>
                    ${job.status === 'failed' ? `<button class="btn btn-sm btn-primary retry-job-btn" data-job-id="${job.id}">Retry</button>` : ''}
                    <button class="btn btn-sm btn-danger delete-job-btn" data-job-id="${job.id}">Delete</button>
                </td>
            </tr>
        `).join('');
    }

    async updateThroughputChart(period) {
        try {
            const response = await this.apiCall(`/api/stats/throughput?period=${period}`);
            if (response.success && this.charts.throughput) {
                const data = response.data.datapoints || [];
                
                this.charts.throughput.data.labels = data.map(point => 
                    new Date(point.timestamp).toLocaleTimeString()
                );
                this.charts.throughput.data.datasets[0].data = data.map(point => point.value);
                this.charts.throughput.update();
            }
        } catch (error) {
            console.error('Failed to update throughput chart:', error);
        }
    }

    connectWebSocket() {
        if (this.websocket) {
            this.websocket.close();
        }

        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = `${protocol}//${window.location.host}/ws`;
        
        console.log('Connecting to WebSocket:', wsUrl);
        
        this.websocket = new WebSocket(wsUrl);
        
        this.websocket.onopen = () => {
            console.log('WebSocket connected');
            this.reconnectAttempts = 0;
            this.updateConnectionStatus(true);
        };
        
        this.websocket.onmessage = (event) => {
            try {
                const message = JSON.parse(event.data);
                this.handleWebSocketMessage(message);
            } catch (error) {
                console.error('Failed to parse WebSocket message:', error);
            }
        };
        
        this.websocket.onclose = () => {
            console.log('WebSocket disconnected');
            this.updateConnectionStatus(false);
            this.scheduleReconnect();
        };
        
        this.websocket.onerror = (error) => {
            console.error('WebSocket error:', error);
            this.updateConnectionStatus(false);
        };
    }

    handleWebSocketMessage(message) {
        console.log('WebSocket message received:', message);
        
        switch (message.type) {
            case 'stats_update':
                this.updateOverviewCards(message.data);
                break;
            case 'job_update':
                this.refreshJobsIfVisible();
                break;
            case 'queue_update':
                this.refreshQueuesIfVisible();
                break;
            case 'ping':
                // Respond to ping
                if (this.websocket.readyState === WebSocket.OPEN) {
                    this.websocket.send(JSON.stringify({ type: 'pong' }));
                }
                break;
            default:
                console.log('Unknown WebSocket message type:', message.type);
        }
    }

    scheduleReconnect() {
        if (this.reconnectAttempts < this.maxReconnectAttempts) {
            this.reconnectAttempts++;
            console.log(`Attempting to reconnect WebSocket (${this.reconnectAttempts}/${this.maxReconnectAttempts})...`);
            
            setTimeout(() => {
                this.connectWebSocket();
            }, this.reconnectInterval * this.reconnectAttempts);
        } else {
            console.log('Max reconnection attempts reached');
            this.showError('Connection lost. Please refresh the page.');
        }
    }

    updateConnectionStatus(connected) {
        const indicator = document.getElementById('connectionStatus');
        if (connected) {
            indicator.textContent = 'Connected';
            indicator.className = 'status-indicator';
        } else {
            indicator.textContent = 'Disconnected';
            indicator.className = 'status-indicator disconnected';
        }
    }

    startPeriodicRefresh() {
        // Refresh every 30 seconds as fallback
        this.refreshInterval = setInterval(() => {
            if (!this.websocket || this.websocket.readyState !== WebSocket.OPEN) {
                this.refreshAllData();
            }
        }, 30000);
    }

    async refreshAllData() {
        console.log('Refreshing all data...');
        await this.loadInitialData();
    }

    refreshJobsIfVisible() {
        // Only refresh if jobs section is visible
        if (this.isElementInViewport(document.querySelector('.jobs-section'))) {
            this.loadJobs();
        }
    }

    refreshQueuesIfVisible() {
        // Only refresh if queues section is visible
        if (this.isElementInViewport(document.querySelector('.queues-section'))) {
            this.loadQueues();
        }
    }

    // Modal management
    showAddJobModal() {
        document.getElementById('addJobModal').classList.add('active');
    }

    showJobDetails(jobId) {
        // Load job details and show modal
        this.loadJobDetails(jobId);
    }

    async loadJobDetails(jobId) {
        try {
            const response = await this.apiCall(`/api/jobs/${jobId}`);
            if (response.success) {
                this.displayJobDetails(response.data);
                document.getElementById('jobModal').classList.add('active');
            }
        } catch (error) {
            console.error('Failed to load job details:', error);
            this.showError('Failed to load job details');
        }
    }

    displayJobDetails(job) {
        const container = document.getElementById('jobDetails');
        container.innerHTML = `
            <div class="form-group">
                <label>Job ID</label>
                <div class="font-mono">${job.id}</div>
            </div>
            <div class="form-group">
                <label>Queue</label>
                <div>${this.escapeHtml(job.queue_name)}</div>
            </div>
            <div class="form-group">
                <label>Status</label>
                <div><span class="status-badge status-${job.status}">${job.status}</span></div>
            </div>
            <div class="form-group">
                <label>Priority</label>
                <div><span class="priority-badge priority-${job.priority}">${job.priority}</span></div>
            </div>
            <div class="form-group">
                <label>Payload</label>
                <textarea readonly class="font-mono" rows="6">${JSON.stringify(job.payload, null, 2)}</textarea>
            </div>
            <div class="form-group">
                <label>Created</label>
                <div>${new Date(job.created_at).toLocaleString()}</div>
            </div>
            ${job.error_message ? `
                <div class="form-group">
                    <label>Error</label>
                    <textarea readonly rows="3">${this.escapeHtml(job.error_message)}</textarea>
                </div>
            ` : ''}
        `;

        // Update modal action buttons
        document.getElementById('retryJobBtn').dataset.jobId = job.id;
        document.getElementById('deleteJobBtn').dataset.jobId = job.id;
        document.getElementById('retryJobBtn').style.display = job.status === 'failed' ? 'inline-flex' : 'none';
    }

    hideModal(modal) {
        modal.classList.remove('active');
    }

    hideAllModals() {
        document.querySelectorAll('.modal').forEach(modal => {
            modal.classList.remove('active');
        });
    }

    // Job actions
    async submitNewJob() {
        const form = document.getElementById('addJobForm');
        const formData = new FormData(form);
        
        try {
            const payload = document.getElementById('jobPayload').value;
            let parsedPayload = {};
            
            if (payload.trim()) {
                parsedPayload = JSON.parse(payload);
            }
            
            const jobData = {
                queue_name: document.getElementById('jobQueue').value,
                priority: document.getElementById('jobPriority').value,
                payload: parsedPayload,
                scheduled_at: document.getElementById('jobScheduledAt').value || null
            };
            
            const response = await this.apiCall('/api/jobs', 'POST', jobData);
            
            if (response.success) {
                this.hideModal(document.getElementById('addJobModal'));
                this.showSuccess('Job added successfully');
                this.loadJobs();
                form.reset();
            } else {
                this.showError(response.error || 'Failed to add job');
            }
        } catch (error) {
            console.error('Failed to submit job:', error);
            this.showError('Invalid JSON payload or network error');
        }
    }

    async retryJob(jobId) {
        try {
            const response = await this.apiCall(`/api/jobs/${jobId}/retry`, 'POST');
            if (response.success) {
                this.showSuccess('Job queued for retry');
                this.loadJobs();
                this.hideAllModals();
            } else {
                this.showError(response.error || 'Failed to retry job');
            }
        } catch (error) {
            console.error('Failed to retry job:', error);
            this.showError('Failed to retry job');
        }
    }

    async deleteJob(jobId) {
        if (!confirm('Are you sure you want to delete this job?')) {
            return;
        }
        
        try {
            const response = await this.apiCall(`/api/jobs/${jobId}`, 'DELETE');
            if (response.success) {
                this.showSuccess('Job deleted successfully');
                this.loadJobs();
                this.hideAllModals();
            } else {
                this.showError(response.error || 'Failed to delete job');
            }
        } catch (error) {
            console.error('Failed to delete job:', error);
            this.showError('Failed to delete job');
        }
    }

    // Queue actions
    async clearQueue(queueName) {
        if (!confirm(`Clear all jobs from queue "${queueName}"?`)) {
            return;
        }
        
        try {
            const response = await this.apiCall(`/api/queues/${encodeURIComponent(queueName)}/clear`, 'POST');
            if (response.success) {
                this.showSuccess('Queue cleared successfully');
                this.loadQueues();
                this.loadJobs();
            } else {
                this.showError(response.error || 'Failed to clear queue');
            }
        } catch (error) {
            console.error('Failed to clear queue:', error);
            this.showError('Failed to clear queue');
        }
    }

    async pauseQueue(queueName) {
        try {
            const response = await this.apiCall(`/api/queues/${encodeURIComponent(queueName)}/pause`, 'POST');
            if (response.success) {
                this.showSuccess('Queue paused successfully');
                this.loadQueues();
            } else {
                this.showError(response.error || 'Failed to pause queue');
            }
        } catch (error) {
            console.error('Failed to pause queue:', error);
            this.showError('Failed to pause queue');
        }
    }

    // Filtering
    filterQueues(searchTerm) {
        const rows = document.querySelectorAll('#queuesTable tbody tr:not(.loading-row)');
        const term = searchTerm.toLowerCase();
        
        rows.forEach(row => {
            const queueName = row.cells[0].textContent.toLowerCase();
            row.style.display = queueName.includes(term) ? '' : 'none';
        });
    }

    // Utility functions
    async apiCall(url, method = 'GET', data = null) {
        const options = {
            method,
            headers: {
                'Content-Type': 'application/json',
            }
        };
        
        // Add authentication header if credentials are available
        if (this.authCredentials) {
            options.headers['Authorization'] = `Basic ${this.authCredentials}`;
        }
        
        if (data) {
            options.body = JSON.stringify(data);
        }
        
        try {
            const response = await fetch(url, options);
            
            // Handle authentication errors
            if (response.status === 401) {
                // Clear invalid credentials
                this.authCredentials = null;
                
                // Prompt for new credentials
                const newCredentials = await this.promptForCredentials();
                if (newCredentials) {
                    this.authCredentials = newCredentials;
                    // Retry the request with new credentials
                    return this.apiCall(url, method, data);
                } else {
                    throw new Error('Authentication required');
                }
            }
            
            if (!response.ok) {
                throw new Error(`HTTP ${response.status}: ${response.statusText}`);
            }
            
            return await response.json();
        } catch (error) {
            if (error.message === 'Authentication required') {
                throw error;
            }
            // For network errors, also try to prompt for auth if we don't have credentials
            if (!this.authCredentials && !url.includes('/health')) {
                const credentials = await this.promptForCredentials();
                if (credentials) {
                    this.authCredentials = credentials;
                    return this.apiCall(url, method, data);
                }
            }
            throw error;
        }
    }

    formatNumber(num) {
        if (num >= 1000000) {
            return (num / 1000000).toFixed(1) + 'M';
        } else if (num >= 1000) {
            return (num / 1000).toFixed(1) + 'K';
        }
        return num.toString();
    }

    formatPercentage(value) {
        return (value * 100).toFixed(1) + '%';
    }

    formatDuration(ms) {
        if (ms < 1000) {
            return ms + 'ms';
        } else if (ms < 60000) {
            return (ms / 1000).toFixed(1) + 's';
        } else {
            return (ms / 60000).toFixed(1) + 'm';
        }
    }

    formatRelativeTime(timestamp) {
        const now = new Date();
        const date = new Date(timestamp);
        const diff = now - date;
        
        const seconds = Math.floor(diff / 1000);
        const minutes = Math.floor(seconds / 60);
        const hours = Math.floor(minutes / 60);
        const days = Math.floor(hours / 24);
        
        if (days > 0) return `${days}d ago`;
        if (hours > 0) return `${hours}h ago`;
        if (minutes > 0) return `${minutes}m ago`;
        return `${seconds}s ago`;
    }

    escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    isElementInViewport(el) {
        const rect = el.getBoundingClientRect();
        return (
            rect.top >= 0 &&
            rect.left >= 0 &&
            rect.bottom <= (window.innerHeight || document.documentElement.clientHeight) &&
            rect.right <= (window.innerWidth || document.documentElement.clientWidth)
        );
    }

    showSuccess(message) {
        this.showNotification(message, 'success');
    }

    showError(message) {
        this.showNotification(message, 'error');
    }

    showNotification(message, type) {
        // Create notification element
        const notification = document.createElement('div');
        notification.className = `notification notification-${type}`;
        notification.textContent = message;
        
        // Add to page
        document.body.appendChild(notification);
        
        // Remove after 5 seconds
        setTimeout(() => {
            if (notification.parentNode) {
                notification.parentNode.removeChild(notification);
            }
        }, 5000);
    }

    destroy() {
        if (this.websocket) {
            this.websocket.close();
        }
        
        if (this.refreshInterval) {
            clearInterval(this.refreshInterval);
        }
        
        // Destroy charts
        Object.values(this.charts).forEach(chart => {
            if (chart) chart.destroy();
        });
    }
}

// Initialize dashboard when DOM is loaded
let dashboard;

document.addEventListener('DOMContentLoaded', () => {
    dashboard = new HammerworkDashboard();
});

// Cleanup on page unload
window.addEventListener('beforeunload', () => {
    if (dashboard) {
        dashboard.destroy();
    }
});

// Expose dashboard globally for debugging
window.dashboard = dashboard;