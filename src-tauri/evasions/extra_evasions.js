// Extra anti-detection evasions — injected via Page.addScriptToEvaluateOnNewDocument
// These supplement ChaserProfile's bootstrap_script (~12 evasions) with ~25 more.
// Runs AFTER bootstrap (which runs first via apply_profile), persists across navigations.
//
// ChaserProfile already handles: webdriver, platform, hardwareConcurrency, deviceMemory,
// maxTouchPoints, WebGL vendor/renderer, userAgentData, video codecs, chrome object stubs,
// CDP marker cleanup. We add everything else.
//
// IMPORTANT: toString() infrastructure is set up FIRST so all overridden functions
// appear as [native code] when inspected. This is ADDITIVE — it preserves any
// existing toString override from the bootstrap script.

(function() {
    'use strict';

    // === 0. toString() Native Function Fix (MUST BE FIRST) ===
    // Set up infrastructure to make overridden functions appear native.
    // Uses the CURRENT toString (which may already be patched by bootstrap)
    // as fallback, so bootstrap's registrations are preserved.
    const _prevToString = Function.prototype.toString;
    const _nativeMap = new Map();

    const makeNative = (fn, name) => {
        if (fn) _nativeMap.set(fn, `function ${name}() { [native code] }`);
    };

    try {
        Function.prototype.toString = function() {
            if (_nativeMap.has(this)) return _nativeMap.get(this);
            return _prevToString.call(this);
        };
        makeNative(Function.prototype.toString, 'toString');
    } catch(e) {}

    // === 1. Navigator Plugins (Chrome has 5 default plugins) ===
    try {
        const pluginData = [
            { name: 'Chrome PDF Plugin', filename: 'internal-pdf-viewer', description: 'Portable Document Format', mimeType: 'application/x-google-chrome-pdf' },
            { name: 'Chrome PDF Viewer', filename: 'mhjfbmdgcfjbbpaeojofohoefgiehjai', description: '', mimeType: 'application/pdf' },
            { name: 'Native Client', filename: 'internal-nacl-plugin', description: '', mimeType: 'application/x-nacl' },
            { name: 'Chromium PDF Plugin', filename: 'internal-pdf-viewer', description: 'Portable Document Format', mimeType: 'application/x-google-chrome-pdf' },
            { name: 'Chromium PDF Viewer', filename: 'mhjfbmdgcfjbbpaeojofohoefgiehjai', description: '', mimeType: 'application/pdf' },
        ];
        const plugins = Object.create(PluginArray.prototype);
        for (let i = 0; i < pluginData.length; i++) {
            const p = Object.create(Plugin.prototype);
            Object.defineProperties(p, {
                name: { get: () => pluginData[i].name, enumerable: true },
                filename: { get: () => pluginData[i].filename, enumerable: true },
                description: { get: () => pluginData[i].description, enumerable: true },
                length: { get: () => 1, enumerable: true }
            });
            Object.defineProperty(plugins, i, { get: () => p, enumerable: true });
            Object.defineProperty(plugins, pluginData[i].name, { get: () => p });
        }
        Object.defineProperty(plugins, 'length', { get: () => pluginData.length, enumerable: true });
        Object.defineProperty(plugins, 'refresh', { value: () => {} });
        Object.defineProperty(Navigator.prototype, 'plugins', { get: () => plugins, configurable: true });
    } catch(e) {}

    // === 2. Navigator MimeTypes ===
    try {
        const mimeData = [
            { type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format' },
            { type: 'application/x-google-chrome-pdf', suffixes: 'pdf', description: 'Portable Document Format' },
            { type: 'application/x-nacl', suffixes: '', description: 'Native Client Executable' },
            { type: 'application/x-pnacl', suffixes: '', description: 'Portable Native Client Executable' },
        ];
        const mimeTypes = Object.create(MimeTypeArray.prototype);
        for (let i = 0; i < mimeData.length; i++) {
            const m = Object.create(MimeType.prototype);
            Object.defineProperties(m, {
                type: { get: () => mimeData[i].type, enumerable: true },
                suffixes: { get: () => mimeData[i].suffixes, enumerable: true },
                description: { get: () => mimeData[i].description, enumerable: true },
            });
            Object.defineProperty(mimeTypes, i, { get: () => m, enumerable: true });
            Object.defineProperty(mimeTypes, mimeData[i].type, { get: () => m });
        }
        Object.defineProperty(mimeTypes, 'length', { get: () => mimeData.length, enumerable: true });
        Object.defineProperty(Navigator.prototype, 'mimeTypes', { get: () => mimeTypes, configurable: true });
    } catch(e) {}

    // === 3. Navigator Languages (match Accept-Language header) ===
    try {
        Object.defineProperty(Navigator.prototype, 'languages', {
            get: () => Object.freeze(['ar-SA', 'ar', 'en-US', 'en']),
            configurable: true
        });
        Object.defineProperty(Navigator.prototype, 'language', {
            get: () => 'ar-SA',
            configurable: true
        });
    } catch(e) {}

    // === 4. Permissions API ===
    try {
        const origQuery = window.Permissions.prototype.query;
        window.Permissions.prototype.query = function(parameters) {
            if (parameters.name === 'notifications') {
                return Promise.resolve({ state: Notification.permission });
            }
            return origQuery.apply(this, arguments);
        };
        makeNative(window.Permissions.prototype.query, 'query');
    } catch(e) {}

    // === 5. Connection API ===
    try {
        const conn = {
            effectiveType: '4g',
            rtt: 50,
            downlink: 10,
            saveData: false,
            type: 'wifi',
            onchange: null,
            addEventListener: function() {},
            removeEventListener: function() {},
            dispatchEvent: function() { return true; }
        };
        Object.defineProperty(Navigator.prototype, 'connection', {
            get: () => conn,
            configurable: true
        });
    } catch(e) {}

    // === 6. Battery API ===
    try {
        if (navigator.getBattery) {
            Navigator.prototype.getBattery = function() {
                return Promise.resolve({
                    charging: true,
                    chargingTime: 0,
                    dischargingTime: Infinity,
                    level: 1.0,
                    onchargingchange: null,
                    onchargingtimechange: null,
                    ondischargingtimechange: null,
                    onlevelchange: null,
                    addEventListener: function() {},
                    removeEventListener: function() {},
                    dispatchEvent: function() { return true; }
                });
            };
            makeNative(Navigator.prototype.getBattery, 'getBattery');
        }
    } catch(e) {}

    // === 7. Screen Properties (match window size) ===
    try {
        Object.defineProperty(Screen.prototype, 'width', { get: () => 1920 });
        Object.defineProperty(Screen.prototype, 'height', { get: () => 1080 });
        Object.defineProperty(Screen.prototype, 'availWidth', { get: () => 1920 });
        Object.defineProperty(Screen.prototype, 'availHeight', { get: () => 1040 });
        Object.defineProperty(Screen.prototype, 'colorDepth', { get: () => 24 });
        Object.defineProperty(Screen.prototype, 'pixelDepth', { get: () => 24 });
    } catch(e) {}

    // === 8. Timezone Override (Asia/Riyadh = UTC+3 = offset -180) ===
    try {
        Date.prototype.getTimezoneOffset = function() { return -180; };
        makeNative(Date.prototype.getTimezoneOffset, 'getTimezoneOffset');

        const origResolvedOptions = Intl.DateTimeFormat.prototype.resolvedOptions;
        Intl.DateTimeFormat.prototype.resolvedOptions = function() {
            const result = origResolvedOptions.apply(this, arguments);
            result.timeZone = 'Asia/Riyadh';
            return result;
        };
        makeNative(Intl.DateTimeFormat.prototype.resolvedOptions, 'resolvedOptions');
    } catch(e) {}

    // === 9. Canvas Fingerprint Noise ===
    try {
        const origToDataURL = HTMLCanvasElement.prototype.toDataURL;
        HTMLCanvasElement.prototype.toDataURL = function(type) {
            if (this.width === 0 || this.height === 0) return origToDataURL.apply(this, arguments);
            try {
                const ctx = this.getContext('2d');
                if (ctx) {
                    const imageData = ctx.getImageData(0, 0, Math.min(this.width, 16), Math.min(this.height, 16));
                    for (let i = 0; i < imageData.data.length; i += 4) {
                        imageData.data[i] = Math.max(0, Math.min(255, imageData.data[i] + (Math.random() > 0.5 ? 1 : -1)));
                    }
                    ctx.putImageData(imageData, 0, 0);
                }
            } catch(ex) {}
            return origToDataURL.apply(this, arguments);
        };
        makeNative(HTMLCanvasElement.prototype.toDataURL, 'toDataURL');

        const origGetImageData = CanvasRenderingContext2D.prototype.getImageData;
        CanvasRenderingContext2D.prototype.getImageData = function() {
            const data = origGetImageData.apply(this, arguments);
            for (let i = 0; i < Math.min(data.data.length, 64); i += 4) {
                data.data[i] = Math.max(0, Math.min(255, data.data[i] + (Math.random() > 0.5 ? 1 : -1)));
            }
            return data;
        };
        makeNative(CanvasRenderingContext2D.prototype.getImageData, 'getImageData');
    } catch(e) {}

    // === 10. AudioContext Fingerprint Noise ===
    try {
        const origGetChannelData = AudioBuffer.prototype.getChannelData;
        AudioBuffer.prototype.getChannelData = function() {
            const result = origGetChannelData.apply(this, arguments);
            if (result && result.length > 0) {
                for (let i = 0; i < Math.min(10, result.length); i++) {
                    result[i] += (Math.random() - 0.5) * 0.0001;
                }
            }
            return result;
        };
        makeNative(AudioBuffer.prototype.getChannelData, 'getChannelData');
    } catch(e) {}

    // === 11. WebGL Complete Fingerprint Spoofing ===
    // Comprehensive WebGL spoofing to hide VNC/software rendering detection
    try {
        const spoofWebGL = (proto) => {
            // Spoof getParameter
            const getParameter = proto.getParameter;
            proto.getParameter = function(parameter) {
                // UNMASKED_VENDOR_WEBGL
                if (parameter === 37445) return 'Google Inc. (NVIDIA Corporation)';
                // UNMASKED_RENDERER_WEBGL — realistic NVIDIA renderer
                if (parameter === 37446) return 'ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER (0x000021C4) Direct3D11 vs_5_0 ps_5_0, D3D11)';
                // MAX_TEXTURE_SIZE - match real GPU
                if (parameter === 3379) return 16384;
                // MAX_RENDERBUFFER_SIZE
                if (parameter === 34024) return 16384;
                // MAX_VIEWPORT_DIMS
                if (parameter === 3386) return new Int32Array([32767, 32767]);
                // MAX_VERTEX_ATTRIBS
                if (parameter === 34921) return 16;
                // MAX_VERTEX_UNIFORM_VECTORS
                if (parameter === 36347) return 4096;
                // MAX_FRAGMENT_UNIFORM_VECTORS
                if (parameter === 36349) return 1024;
                // MAX_VARYING_VECTORS
                if (parameter === 36348) return 30;
                // ALIASED_LINE_WIDTH_RANGE
                if (parameter === 33902) return new Float32Array([1, 1]);
                // ALIASED_POINT_SIZE_RANGE
                if (parameter === 33901) return new Float32Array([1, 1024]);
                return getParameter.apply(this, arguments);
            };
            makeNative(proto.getParameter, 'getParameter');

            // Spoof getShaderPrecisionFormat - critical for fingerprinting
            const getShaderPrecisionFormat = proto.getShaderPrecisionFormat;
            proto.getShaderPrecisionFormat = function(shaderType, precisionType) {
                // Return consistent values matching real NVIDIA GPU
                return {
                    rangeMin: 127,
                    rangeMax: 127,
                    precision: 23
                };
            };
            makeNative(proto.getShaderPrecisionFormat, 'getShaderPrecisionFormat');

            // Spoof getSupportedExtensions - return realistic extension list
            const getSupportedExtensions = proto.getSupportedExtensions;
            proto.getSupportedExtensions = function() {
                return [
                    'ANGLE_instanced_arrays', 'EXT_blend_minmax', 'EXT_color_buffer_half_float',
                    'EXT_disjoint_timer_query', 'EXT_float_blend', 'EXT_frag_depth',
                    'EXT_shader_texture_lod', 'EXT_texture_compression_bptc',
                    'EXT_texture_compression_rgtc', 'EXT_texture_filter_anisotropic',
                    'EXT_sRGB', 'KHR_parallel_shader_compile', 'OES_element_index_uint',
                    'OES_fbo_render_mipmap', 'OES_standard_derivatives', 'OES_texture_float',
                    'OES_texture_float_linear', 'OES_texture_half_float',
                    'OES_texture_half_float_linear', 'OES_vertex_array_object',
                    'WEBGL_color_buffer_float', 'WEBGL_compressed_texture_s3tc',
                    'WEBGL_compressed_texture_s3tc_srgb', 'WEBGL_debug_renderer_info',
                    'WEBGL_debug_shaders', 'WEBGL_depth_texture', 'WEBGL_draw_buffers',
                    'WEBGL_lose_context', 'WEBGL_multi_draw'
                ];
            };
            makeNative(proto.getSupportedExtensions, 'getSupportedExtensions');

            // Spoof getContextAttributes
            const getContextAttributes = proto.getContextAttributes;
            proto.getContextAttributes = function() {
                return {
                    alpha: true,
                    antialias: true,
                    depth: true,
                    desynchronized: false,
                    failIfMajorPerformanceCaveat: false,
                    powerPreference: 'default',
                    premultipliedAlpha: true,
                    preserveDrawingBuffer: false,
                    stencil: false,
                    xrCompatible: false
                };
            };
            makeNative(proto.getContextAttributes, 'getContextAttributes');
        };

        spoofWebGL(WebGLRenderingContext.prototype);
        if (typeof WebGL2RenderingContext !== 'undefined') {
            spoofWebGL(WebGL2RenderingContext.prototype);
        }
    } catch(e) {}

    // === 12. Notification Permission ===
    try {
        Object.defineProperty(Notification, 'permission', {
            get: () => 'default',
            configurable: true
        });
    } catch(e) {}

    // === 13. Performance Memory Mock ===
    try {
        if (window.performance) {
            Object.defineProperty(performance, 'memory', {
                get: () => ({
                    jsHeapSizeLimit: 2172649472,
                    totalJSHeapSize: 35839491,
                    usedJSHeapSize: 24290498
                }),
                configurable: true
            });
        }
    } catch(e) {}

    // === 14. Error Stack Trace Cleanup ===
    try {
        const origPrepareStackTrace = Error.prepareStackTrace;
        Error.prepareStackTrace = function(err, stack) {
            if (origPrepareStackTrace) {
                const result = origPrepareStackTrace(err, stack);
                if (typeof result === 'string') {
                    return result.replace(/chrome-extension:\/\/[^\s]+/g, '')
                                 .replace(/__puppeteer_evaluation_script__/g, '')
                                 .replace(/__playwright_evaluation_script__/g, '');
                }
                return result;
            }
            return err.stack;
        };
    } catch(e) {}

    // === 15. WebRTC IP Leak Protection ===
    try {
        const origRTCPeerConnection = window.RTCPeerConnection || window.webkitRTCPeerConnection;
        if (origRTCPeerConnection) {
            window.RTCPeerConnection = function() {
                const pc = new origRTCPeerConnection(...arguments);
                const origCreateOffer = pc.createOffer.bind(pc);
                pc.createOffer = function(options) {
                    if (options) options.offerToReceiveAudio = false;
                    return origCreateOffer(options);
                };
                return pc;
            };
            window.RTCPeerConnection.prototype = origRTCPeerConnection.prototype;
        }
    } catch(e) {}

    // === 16. Document Visibility ===
    try {
        Object.defineProperty(document, 'hidden', { get: () => false, configurable: true });
        Object.defineProperty(document, 'visibilityState', { get: () => 'visible', configurable: true });
    } catch(e) {}

    // === 17. Document hasFocus ===
    try {
        Document.prototype.hasFocus = function() { return true; };
        makeNative(Document.prototype.hasFocus, 'hasFocus');
    } catch(e) {}

    // === 18. doNotTrack ===
    try {
        Object.defineProperty(Navigator.prototype, 'doNotTrack', {
            get: () => null, // Most users don't set DNT
            configurable: true
        });
    } catch(e) {}

    // === 19. Speech Synthesis Voices (empty — consistent) ===
    try {
        if (window.speechSynthesis) {
            speechSynthesis.getVoices = function() { return []; };
            makeNative(speechSynthesis.getVoices, 'getVoices');
        }
    } catch(e) {}

    // === 20. Window Outer Dimensions ===
    try {
        Object.defineProperty(window, 'outerWidth', { get: () => 1920 });
        Object.defineProperty(window, 'outerHeight', { get: () => 1080 });
    } catch(e) {}

    // === 21. Screen Orientation ===
    try {
        if (window.screen && window.screen.orientation) {
            Object.defineProperty(screen.orientation, 'type', { get: () => 'landscape-primary' });
            Object.defineProperty(screen.orientation, 'angle', { get: () => 0 });
        }
    } catch(e) {}

    // === 22. Console DevTools Detection Bypass ===
    try {
        Object.defineProperty(window, '__REACT_DEVTOOLS_GLOBAL_HOOK__', { get: () => undefined });
    } catch(e) {}

    // === 23. Performance.now() Noise (prevents timing attacks) ===
    try {
        const origNow = Performance.prototype.now;
        Performance.prototype.now = function() {
            return origNow.call(this) + (Math.random() * 0.1);
        };
        makeNative(Performance.prototype.now, 'now');
    } catch(e) {}

    // === 24. Storage Quota Randomization ===
    try {
        if (navigator.storage && navigator.storage.estimate) {
            const origEstimate = navigator.storage.estimate.bind(navigator.storage);
            navigator.storage.estimate = async function() {
                const result = await origEstimate();
                result.quota = 2147483648 + Math.floor(Math.random() * 1073741824); // 2-3 GB
                return result;
            };
        }
    } catch(e) {}

    // === 25. Bluetooth/USB/Serial/HID/Gamepad API Stubs ===
    try {
        if (navigator.bluetooth) {
            navigator.bluetooth.getAvailability = () => Promise.resolve(false);
        }
        if (navigator.usb) {
            navigator.usb.getDevices = () => Promise.resolve([]);
        }
        if (navigator.serial) {
            navigator.serial.getPorts = () => Promise.resolve([]);
        }
        if (navigator.hid) {
            navigator.hid.getDevices = () => Promise.resolve([]);
        }
        if (navigator.getGamepads) {
            Navigator.prototype.getGamepads = function() { return [null, null, null, null]; };
            makeNative(Navigator.prototype.getGamepads, 'getGamepads');
        }
    } catch(e) {}

    // === 26. CDP Runtime/Debugger Detection Hide ===
    try {
        // Remove CDP-related properties that leak automation
        const cdpProps = ['__cdc_', '__driver_', '__webdriver_', '__selenium_',
                         '$chrome_asyncScriptInfo', '$cdc_'];
        for (const prop of cdpProps) {
            for (const key of Object.keys(document)) {
                if (key.startsWith(prop) || key.includes('cdc_')) {
                    delete document[key];
                }
            }
            for (const key of Object.keys(window)) {
                if (key.startsWith(prop) || key.includes('cdc_')) {
                    delete window[key];
                }
            }
        }
    } catch(e) {}

    // === 27. Media Codecs (canPlayType) ===
    try {
        const origCanPlayType = HTMLVideoElement.prototype.canPlayType;
        HTMLVideoElement.prototype.canPlayType = function(type) {
            // Ensure common codecs return expected values
            if (type.includes('h264') || type.includes('avc1')) return 'probably';
            if (type.includes('vp8') || type.includes('vp9')) return 'probably';
            if (type.includes('opus') || type.includes('vorbis')) return 'probably';
            if (type.includes('mp4')) return 'probably';
            if (type.includes('webm')) return 'probably';
            return origCanPlayType.call(this, type);
        };
        makeNative(HTMLVideoElement.prototype.canPlayType, 'canPlayType');
    } catch(e) {}

})();
