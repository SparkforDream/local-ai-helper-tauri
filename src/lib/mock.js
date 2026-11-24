// 浏览器预览用的假数据。Tauri 里走真实采集，不会读这里。
window.__MOCK__ = {
  schemaVersion: 1,
  collectedAt: new Date().toISOString(),
  cpu: {
    name: "13th Gen Intel(R) Core(TM) i7-13650HX",
    physicalCores: 14,
    logicalCores: 20,
    baseMhz: 2600,
    avx2: true
  },
  memory: {
    totalBytes: 16780566528,
    visibleBytes: 16780566528,
    freeBytes: 1785184256,
    moduleCount: 2,
    moduleSpeedMhz: 4800,
    largestModuleGb: 8
  },
  gpus: [
    { name: "MuMu Virtual Display Adapter", driverVersion: "20.36.41.498", vramBytes: null, vramSource: null, vendor: "other" },
    { name: "NVIDIA GeForce RTX 4060 Laptop GPU", driverVersion: "32.0.16.2124", vramBytes: 8585740288, vramSource: "nvidia-smi", vendor: "nvidia" },
    { name: "Intel(R) UHD Graphics", driverVersion: "32.0.101.7026", vramBytes: 2147479552, vramSource: "wmi.AdapterRAM(capped)", vendor: "intel" }
  ],
  disks: [
    { letter: "C:", label: "Spark", totalBytes: 268959617024, freeBytes: 138905157632 },
    { letter: "D:", label: "Blossom", totalBytes: 727868100608, freeBytes: 227038265344 }
  ],
  environment: {
    caption: "Microsoft Windows 11 专业工作站版",
    version: "10.0.26300",
    build: 26300,
    arch: "64 位",
    isWin11: true,
    manufacturer: "ASUSTeK COMPUTER INC.",
    model: "ROG Strix G614JV_G614JV",
    webview2: "157.0.4341.46"
  },
  notes: [],
  elapsedMs: 585
};
