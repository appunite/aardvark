Operating System: macOS
CPU Information: Apple M3 Max
Number of Available Cores: 16

Commit: e97e04e

Perf run with command:
```bash
cargo run -p aardvark-perf -- all --iterations 5 --json target/perf/results.json
```

### Echo
#### Profile: high
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1016.50 |  989.64 | 1060.97 |  24.35 | 1009.87 | 1052.78 | 1059.33 |   1529.72 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 |  985.37 |  985.37 |  985.37 |   0.00 |  985.37 |  985.37 |  985.37 |   1399.06 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  106.56 |   71.57 |  240.87 |  67.17 |   73.13 |  207.63 |  234.22 |   1399.06 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1002.95 | 1002.95 | 1002.95 |   0.00 | 1002.95 | 1002.95 | 1002.95 |   1407.64 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  105.57 |   69.34 |  242.85 |  68.66 |   71.82 |  209.01 |  236.08 |   1407.64 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 |  990.89 |  990.89 |  990.89 |   0.00 |  990.89 |  990.89 |  990.89 |   1403.34 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  108.95 |   69.80 |  250.47 |  70.79 |   74.65 |  215.71 |  243.52 |   1403.34 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1011.49 | 1011.49 | 1011.49 |   0.00 | 1011.49 | 1011.49 | 1011.49 |   1391.55 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  212.21 |  201.96 |  240.30 |  14.17 |  205.71 |  233.82 |  239.00 |   1391.55 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1052.18 | 1052.18 | 1052.18 |   0.00 | 1052.18 | 1052.18 | 1052.18 |   1428.61 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  253.41 |  247.95 |  263.01 |   5.43 |  252.92 |  261.33 |  262.67 |   1428.61 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 |  928.64 |  915.12 |  940.38 |   8.32 |  928.03 |  938.94 |  940.10 |   1402.25 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 |  951.19 |  951.19 |  951.19 |   0.00 |  951.19 |  951.19 |  951.19 |   1406.56 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |   51.98 |   16.99 |  188.02 |  68.03 |   17.81 |  154.38 |  181.29 |   1406.56 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 |  941.30 |  941.30 |  941.30 |   0.00 |  941.30 |  941.30 |  941.30 |   1412.95 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |   52.15 |   17.11 |  188.40 |  68.13 |   18.06 |  154.67 |  181.65 |   1412.95 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 |  913.78 |  913.78 |  913.78 |   0.00 |  913.78 |  913.78 |  913.78 |   1405.36 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |   53.65 |   17.95 |  190.38 |  68.38 |   20.06 |  156.63 |  183.63 |   1405.36 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 |  944.04 |  944.04 |  944.04 |   0.00 |  944.04 |  944.04 |  944.04 |   1410.80 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  153.93 |  142.30 |  190.04 |  18.11 |  145.95 |  181.31 |  188.29 |   1410.80 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 |  927.19 |  927.19 |  927.19 |   0.00 |  927.19 |  927.19 |  927.19 |   1403.06 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  188.15 |  184.97 |  191.64 |   2.50 |  189.16 |  191.19 |  191.55 |   1403.06 |
| HostPython                     | -          | -              | -                   |    5 |    0.00 |    0.00 |    0.00 |   0.00 |    0.00 |    0.00 |    0.00 |     19.09 |

#### Profile: low
| Mode                           | Invocation | Path           | Cleanup             | Iter | Avg ms | Min ms | Max ms | Std ms | P50 ms | P95 ms | P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | -----: | -----: | -----: | -----: | -----: | -----: | -----: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 905.57 | 899.16 | 913.04 |   5.01 | 907.26 | 911.93 | 912.82 |   1605.66 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 896.75 | 896.75 | 896.75 |   0.00 | 896.75 | 896.75 | 896.75 |   1601.23 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  37.44 |   4.75 | 165.43 |  64.00 |   5.17 | 133.73 | 159.09 |   1601.23 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 896.73 | 896.73 | 896.73 |   0.00 | 896.73 | 896.73 | 896.73 |   1596.73 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  37.32 |   4.59 | 165.79 |  64.24 |   4.99 | 133.89 | 159.41 |   1596.73 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 904.23 | 904.23 | 904.23 |   0.00 | 904.23 | 904.23 | 904.23 |   1598.58 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  37.86 |   4.91 | 165.33 |  63.74 |   6.84 | 133.65 | 158.99 |   1598.58 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 900.32 | 900.32 | 900.32 |   0.00 | 900.32 | 900.32 | 900.32 |   1597.95 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 | 138.91 | 129.61 | 165.14 |  13.29 | 133.00 | 159.29 | 163.97 |   1597.95 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 905.24 | 905.24 | 905.24 |   0.00 | 905.24 | 905.24 | 905.24 |   1597.00 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 | 163.96 | 162.33 | 166.54 |   1.61 | 162.91 | 166.26 | 166.48 |   1597.00 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 912.17 | 905.33 | 921.29 |   5.75 | 910.44 | 920.22 | 921.08 |   1600.22 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 950.34 | 950.34 | 950.34 |   0.00 | 950.34 | 950.34 | 950.34 |   1598.62 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  53.21 |  17.58 | 191.63 |  69.22 |  18.21 | 157.40 | 184.79 |   1598.62 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 945.13 | 945.13 | 945.13 |   0.00 | 945.13 | 945.13 | 945.13 |   1607.20 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  50.65 |  16.75 | 182.03 |  65.70 |  17.55 | 149.50 | 175.53 |   1607.20 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 955.76 | 955.76 | 955.76 |   0.00 | 955.76 | 955.76 | 955.76 |   1609.59 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  52.20 |  17.67 | 184.16 |  65.98 |  19.98 | 151.34 | 177.60 |   1609.59 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 918.39 | 918.39 | 918.39 |   0.00 | 918.39 | 918.39 | 918.39 |   1603.95 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 | 159.32 | 144.73 | 185.48 |  15.19 | 151.77 | 181.80 | 184.74 |   1603.95 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 911.33 | 911.33 | 911.33 |   0.00 | 911.33 | 911.33 | 911.33 |   1599.11 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 | 184.71 | 181.63 | 188.80 |   2.77 | 184.14 | 188.42 | 188.72 |   1599.11 |
| HostPython                     | -          | -              | -                   |    5 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |     16.44 |

#### Profile: medium
| Mode                           | Invocation | Path           | Cleanup             | Iter | Avg ms | Min ms | Max ms | Std ms | P50 ms | P95 ms | P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | -----: | -----: | -----: | -----: | -----: | -----: | -----: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 898.70 | 894.06 | 902.13 |   2.65 | 899.33 | 901.65 | 902.04 |   1487.73 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 905.80 | 905.80 | 905.80 |   0.00 | 905.80 | 905.80 | 905.80 |   1551.52 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  37.50 |   5.16 | 162.75 |  62.63 |   6.52 | 131.53 | 156.50 |   1551.52 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 905.15 | 905.15 | 905.15 |   0.00 | 905.15 | 905.15 | 905.15 |   1552.58 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  37.08 |   4.81 | 164.24 |  63.58 |   5.05 | 132.66 | 157.93 |   1552.58 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 914.66 | 914.66 | 914.66 |   0.00 | 914.66 | 914.66 | 914.66 |   1556.77 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  37.70 |   4.80 | 167.36 |  64.83 |   5.15 | 135.15 | 160.91 |   1556.77 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 915.26 | 915.26 | 915.26 |   0.00 | 915.26 | 915.26 | 915.26 |   1547.95 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 | 142.54 | 133.65 | 160.73 |   9.92 | 137.45 | 157.64 | 160.11 |   1547.95 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 893.25 | 893.25 | 893.25 |   0.00 | 893.25 | 893.25 | 893.25 |   1521.39 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 | 166.42 | 163.43 | 171.45 |   2.87 | 165.42 | 170.67 | 171.30 |   1521.39 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 911.52 | 907.66 | 914.36 |   2.27 | 912.22 | 914.04 | 914.30 |   1553.36 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 911.24 | 911.24 | 911.24 |   0.00 | 911.24 | 911.24 | 911.24 |   1567.80 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  50.14 |  16.79 | 179.82 |  64.85 |  17.53 | 147.78 | 173.41 |   1567.80 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 913.41 | 913.41 | 913.41 |   0.00 | 913.41 | 913.41 | 913.41 |   1562.97 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  50.35 |  16.68 | 182.04 |  65.85 |  17.20 | 149.44 | 175.52 |   1562.97 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 920.33 | 920.33 | 920.33 |   0.00 | 920.33 | 920.33 | 920.33 |   1561.16 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  50.52 |  16.85 | 181.52 |  65.50 |  17.57 | 149.08 | 175.03 |   1561.16 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 906.83 | 906.83 | 906.83 |   0.00 | 906.83 | 906.83 | 906.83 |   1557.52 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 | 151.59 | 141.24 | 179.40 |  14.14 | 144.15 | 173.36 | 178.19 |   1557.52 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 920.06 | 920.06 | 920.06 |   0.00 | 920.06 | 920.06 | 920.06 |   1552.09 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 | 181.43 | 179.08 | 183.10 |   1.45 | 181.97 | 182.98 | 183.08 |   1552.09 |
| HostPython                     | -          | -              | -                   |    5 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |     16.56 |

#### Profile: none
| Mode                           | Invocation | Path           | Cleanup             | Iter | Avg ms | Min ms | Max ms | Std ms | P50 ms | P95 ms | P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | -----: | -----: | -----: | -----: | -----: | -----: | -----: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 908.76 | 894.17 | 925.80 |  12.50 | 903.58 | 924.89 | 925.62 |    358.95 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 904.08 | 904.08 | 904.08 |   0.00 | 904.08 | 904.08 | 904.08 |    666.64 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  36.09 |   4.61 | 158.89 |  61.40 |   5.10 | 128.52 | 152.81 |    666.64 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 896.41 | 896.41 | 896.41 |   0.00 | 896.41 | 896.41 | 896.41 |    676.48 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  38.49 |   4.42 | 172.10 |  66.80 |   4.92 | 138.95 | 165.47 |    676.48 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 892.18 | 892.18 | 892.18 |   0.00 | 892.18 | 892.18 | 892.18 |    667.06 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  35.88 |   4.53 | 159.02 |  61.57 |   4.88 | 128.45 | 152.91 |    667.06 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 906.03 | 906.03 | 906.03 |   0.00 | 906.03 | 906.03 | 906.03 |    641.78 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 | 136.06 | 128.80 | 156.84 |  10.48 | 131.91 | 152.00 | 155.87 |    641.78 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 897.56 | 897.56 | 897.56 |   0.00 | 897.56 | 897.56 | 897.56 |    487.58 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 | 162.75 | 159.75 | 165.26 |   2.27 | 163.86 | 165.12 | 165.23 |    487.58 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 914.71 | 905.09 | 922.64 |   5.65 | 915.27 | 921.43 | 922.40 |    678.98 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 916.25 | 916.25 | 916.25 |   0.00 | 916.25 | 916.25 | 916.25 |    712.72 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  51.23 |  17.34 | 181.72 |  65.25 |  19.61 | 149.30 | 175.24 |    712.72 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 911.30 | 911.30 | 911.30 |   0.00 | 911.30 | 911.30 | 911.30 |    712.14 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  49.72 |  16.95 | 177.64 |  63.97 |  17.59 | 145.96 | 171.31 |    712.14 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 905.79 | 905.79 | 905.79 |   0.00 | 905.79 | 905.79 | 905.79 |    715.08 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  50.03 |  17.08 | 178.87 |  64.43 |  17.56 | 146.92 | 172.48 |    715.08 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 914.49 | 914.49 | 914.49 |   0.00 | 914.49 | 914.49 | 914.49 |    712.25 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 | 154.65 | 143.45 | 182.43 |  14.61 | 146.09 | 177.21 | 181.38 |    712.25 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 906.10 | 906.10 | 906.10 |   0.00 | 906.10 | 906.10 | 906.10 |    697.64 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 | 187.46 | 181.23 | 196.83 |   5.57 | 187.60 | 195.31 | 196.52 |    697.64 |
| HostPython                     | -          | -              | -                   |    5 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |   0.00 |     16.84 |


### Numpy
#### Profile: high
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1242.75 | 1225.14 | 1303.56 |  30.51 | 1227.45 | 1289.27 | 1300.70 |   1413.81 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1213.52 | 1213.52 | 1213.52 |   0.00 | 1213.52 | 1213.52 | 1213.52 |   1551.55 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  110.53 |   20.05 |  467.20 | 178.34 |   21.87 |  378.38 |  449.44 |   1551.55 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1202.37 | 1202.37 | 1202.37 |   0.00 | 1202.37 | 1202.37 | 1202.37 |   1545.78 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  108.81 |   19.49 |  463.32 | 177.25 |   20.21 |  374.94 |  445.64 |   1545.78 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1242.20 | 1242.20 | 1242.20 |   0.00 | 1242.20 | 1242.20 | 1242.20 |   1553.77 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  108.84 |   19.94 |  460.61 | 175.88 |   21.70 |  372.85 |  443.06 |   1553.77 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1238.03 | 1238.03 | 1238.03 |   0.00 | 1238.03 | 1238.03 | 1238.03 |   1545.33 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  417.64 |  394.37 |  451.78 |  20.34 |  412.22 |  446.93 |  450.81 |   1545.33 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1242.84 | 1242.84 | 1242.84 |   0.00 | 1242.84 | 1242.84 | 1242.84 |   1505.80 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  455.10 |  447.72 |  459.95 |   4.03 |  456.14 |  459.28 |  459.82 |   1505.80 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1264.20 | 1238.55 | 1313.73 |  26.40 | 1256.56 | 1304.12 | 1311.81 |   1545.39 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1233.09 | 1233.09 | 1233.09 |   0.00 | 1233.09 | 1233.09 | 1233.09 |   1541.17 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  129.32 |   40.49 |  478.66 | 174.68 |   42.31 |  391.63 |  461.26 |   1541.17 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1302.45 | 1302.45 | 1302.45 |   0.00 | 1302.45 | 1302.45 | 1302.45 |   1406.50 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  130.69 |   38.19 |  498.41 | 183.86 |   38.58 |  406.67 |  480.06 |   1406.50 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1270.15 | 1270.15 | 1270.15 |   0.00 | 1270.15 | 1270.15 | 1270.15 |   1544.52 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  132.95 |   38.47 |  504.39 | 185.74 |   39.04 |  412.36 |  485.99 |   1544.52 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1277.87 | 1277.87 | 1277.87 |   0.00 | 1277.87 | 1277.87 | 1277.87 |   1545.81 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  432.18 |  411.22 |  497.02 |  32.52 |  417.53 |  481.36 |  493.89 |   1545.81 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1255.74 | 1255.74 | 1255.74 |   0.00 | 1255.74 | 1255.74 | 1255.74 |   1535.02 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  502.65 |  479.32 |  521.02 |  18.22 |  513.76 |  520.30 |  520.88 |   1535.02 |
| HostPython                     | -          | -              | -                   |    5 |    6.83 |    3.70 |   10.11 |   2.51 |    5.90 |    9.97 |   10.09 |    105.28 |

#### Profile: low
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1212.80 | 1207.70 | 1218.87 |   4.21 | 1210.72 | 1218.41 | 1218.78 |   1600.31 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1199.92 | 1199.92 | 1199.92 |   0.00 | 1199.92 | 1199.92 | 1199.92 |   1603.44 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  109.97 |   19.90 |  467.99 | 179.01 |   20.36 |  378.67 |  450.12 |   1603.44 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1267.66 | 1267.66 | 1267.66 |   0.00 | 1267.66 | 1267.66 | 1267.66 |   1610.06 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  111.21 |   20.29 |  468.84 | 178.82 |   21.86 |  379.96 |  451.06 |   1610.06 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1236.09 | 1236.09 | 1236.09 |   0.00 | 1236.09 | 1236.09 | 1236.09 |   1603.45 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  109.30 |   21.71 |  457.55 | 174.13 |   22.66 |  370.60 |  440.16 |   1603.45 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1206.97 | 1206.97 | 1206.97 |   0.00 | 1206.97 | 1206.97 | 1206.97 |   1598.97 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  406.85 |  392.27 |  448.56 |  21.19 |  399.79 |  439.11 |  446.67 |   1598.97 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1221.89 | 1221.89 | 1221.89 |   0.00 | 1221.89 | 1221.89 | 1221.89 |   1602.88 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  457.10 |  450.61 |  464.24 |   4.76 |  457.85 |  463.29 |  464.05 |   1602.88 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1243.32 | 1223.98 | 1258.76 |  11.37 | 1242.80 | 1256.80 | 1258.37 |   1603.80 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1238.88 | 1238.88 | 1238.88 |   0.00 | 1238.88 | 1238.88 | 1238.88 |   1602.86 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  122.50 |   33.04 |  478.07 | 177.79 |   33.73 |  389.25 |  460.31 |   1602.86 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1242.58 | 1242.58 | 1242.58 |   0.00 | 1242.58 | 1242.58 | 1242.58 |   1604.78 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  120.14 |   31.90 |  469.65 | 174.76 |   32.81 |  382.56 |  452.23 |   1604.78 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1224.60 | 1224.60 | 1224.60 |   0.00 | 1224.60 | 1224.60 | 1224.60 |   1613.25 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  121.25 |   32.10 |  473.15 | 175.95 |   33.15 |  385.59 |  455.64 |   1613.25 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1232.70 | 1232.70 | 1232.70 |   0.00 | 1232.70 | 1232.70 | 1232.70 |   1607.94 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  420.31 |  401.88 |  473.82 |  26.90 |  409.01 |  461.04 |  471.26 |   1607.94 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1233.79 | 1233.79 | 1233.79 |   0.00 | 1233.79 | 1233.79 | 1233.79 |   1613.11 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  477.17 |  468.06 |  487.16 |   7.53 |  477.85 |  486.41 |  487.01 |   1613.11 |
| HostPython                     | -          | -              | -                   |    5 |    0.01 |    0.01 |    0.02 |   0.01 |    0.01 |    0.02 |    0.02 |     35.22 |

#### Profile: medium
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1210.99 | 1201.49 | 1217.07 |   5.49 | 1212.66 | 1216.65 | 1216.98 |   1573.78 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1205.72 | 1205.72 | 1205.72 |   0.00 | 1205.72 | 1205.72 | 1205.72 |   1585.33 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  107.12 |   19.32 |  455.73 | 174.31 |   19.78 |  368.84 |  438.35 |   1585.33 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1203.25 | 1203.25 | 1203.25 |   0.00 | 1203.25 | 1203.25 | 1203.25 |   1577.72 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  108.17 |   19.83 |  455.18 | 173.51 |   21.55 |  369.01 |  437.95 |   1577.72 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1201.09 | 1201.09 | 1201.09 |   0.00 | 1201.09 | 1201.09 | 1201.09 |   1586.12 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  107.42 |   19.84 |  455.02 | 173.80 |   20.37 |  368.37 |  437.69 |   1586.12 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1194.11 | 1194.11 | 1194.11 |   0.00 | 1194.11 | 1194.11 | 1194.11 |   1576.92 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  404.33 |  389.35 |  451.72 |  23.87 |  393.19 |  440.87 |  449.55 |   1576.92 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1219.26 | 1219.26 | 1219.26 |   0.00 | 1219.26 | 1219.26 | 1219.26 |   1567.86 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  451.91 |  445.50 |  466.34 |   7.37 |  449.39 |  463.04 |  465.68 |   1567.86 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1232.66 | 1212.97 | 1260.96 |  16.35 | 1230.58 | 1256.21 | 1260.01 |   1579.58 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1247.86 | 1247.86 | 1247.86 |   0.00 | 1247.86 | 1247.86 | 1247.86 |   1581.02 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  119.77 |   31.84 |  467.65 | 173.94 |   32.43 |  381.11 |  450.34 |   1581.02 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1219.23 | 1219.23 | 1219.23 |   0.00 | 1219.23 | 1219.23 | 1219.23 |   1585.12 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  119.63 |   32.11 |  466.22 | 173.30 |   33.06 |  379.79 |  448.93 |   1585.12 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1221.52 | 1221.52 | 1221.52 |   0.00 | 1221.52 | 1221.52 | 1221.52 |   1581.83 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  120.81 |   31.95 |  472.89 | 176.04 |   32.95 |  385.08 |  455.32 |   1581.83 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1217.59 | 1217.59 | 1217.59 |   0.00 | 1217.59 | 1217.59 | 1217.59 |   1575.80 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  419.98 |  401.80 |  473.81 |  27.07 |  406.82 |  461.23 |  471.29 |   1575.80 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1222.03 | 1222.03 | 1222.03 |   0.00 | 1222.03 | 1222.03 | 1222.03 |   1586.31 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  466.72 |  464.01 |  470.28 |   2.33 |  466.14 |  469.91 |  470.21 |   1586.31 |
| HostPython                     | -          | -              | -                   |    5 |    0.02 |    0.02 |    0.04 |   0.01 |    0.02 |    0.03 |    0.04 |     35.77 |

#### Profile: none
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1205.45 | 1195.37 | 1216.30 |   8.44 | 1201.50 | 1215.97 | 1216.24 |    717.95 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1228.42 | 1228.42 | 1228.42 |   0.00 | 1228.42 | 1228.42 | 1228.42 |   1044.31 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  108.96 |   20.11 |  458.28 | 174.67 |   21.10 |  371.48 |  440.92 |   1044.31 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1225.16 | 1225.16 | 1225.16 |   0.00 | 1225.16 | 1225.16 | 1225.16 |   1043.30 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  108.55 |   19.50 |  461.55 | 176.50 |   20.42 |  373.55 |  443.95 |   1043.30 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1206.04 | 1206.04 | 1206.04 |   0.00 | 1206.04 | 1206.04 | 1206.04 |   1052.86 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  106.59 |   19.57 |  451.32 | 172.37 |   20.48 |  365.40 |  434.13 |   1052.86 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1243.79 | 1243.79 | 1243.79 |   0.00 | 1243.79 | 1243.79 | 1243.79 |   1032.05 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  407.32 |  390.22 |  446.77 |  20.64 |  398.75 |  439.01 |  445.21 |   1032.05 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1204.90 | 1204.90 | 1204.90 |   0.00 | 1204.90 | 1204.90 | 1204.90 |    855.62 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  448.10 |  443.59 |  452.20 |   2.78 |  448.48 |  451.54 |  452.07 |    855.62 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1228.15 | 1223.23 | 1234.61 |   4.38 | 1225.99 | 1234.09 | 1234.50 |   1047.55 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1252.43 | 1252.43 | 1252.43 |   0.00 | 1252.43 | 1252.43 | 1252.43 |   1061.41 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  121.88 |   32.15 |  476.14 | 177.13 |   33.10 |  388.00 |  458.51 |   1061.41 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1252.21 | 1252.21 | 1252.21 |   0.00 | 1252.21 | 1252.21 | 1252.21 |   1068.64 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  118.96 |   32.14 |  464.21 | 172.63 |   32.56 |  378.08 |  446.98 |   1068.64 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1221.06 | 1221.06 | 1221.06 |   0.00 | 1221.06 | 1221.06 | 1221.06 |   1058.27 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  119.86 |   31.92 |  467.43 | 173.79 |   32.78 |  380.86 |  450.12 |   1058.27 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1224.07 | 1224.07 | 1224.07 |   0.00 | 1224.07 | 1224.07 | 1224.07 |   1074.70 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  418.90 |  403.39 |  459.20 |  20.69 |  410.47 |  450.69 |  457.50 |   1074.70 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1235.64 | 1235.64 | 1235.64 |   0.00 | 1235.64 | 1235.64 | 1235.64 |   1053.72 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  469.81 |  466.45 |  472.22 |   1.93 |  470.21 |  471.95 |  472.17 |   1053.72 |
| HostPython                     | -          | -              | -                   |    5 |    0.01 |    0.01 |    0.03 |   0.01 |    0.01 |    0.02 |    0.03 |     35.39 |


### Pandas
#### Profile: high
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 2751.34 | 2734.88 | 2769.55 |  11.27 | 2749.09 | 2766.70 | 2768.98 |   1442.05 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 2820.87 | 2820.87 | 2820.87 |   0.00 | 2820.87 | 2820.87 | 2820.87 |   1498.05 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  445.36 |   74.98 | 1910.31 | 732.48 |   80.72 | 1544.86 | 1837.22 |   1498.05 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 2838.84 | 2838.84 | 2838.84 |   0.00 | 2838.84 | 2838.84 | 2838.84 |   1441.50 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  448.46 |   73.98 | 1943.13 | 747.33 |   75.10 | 1569.59 | 1868.42 |   1441.50 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 2727.01 | 2727.01 | 2727.01 |   0.00 | 2727.01 | 2727.01 | 2727.01 |   1508.30 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  446.86 |   73.70 | 1936.79 | 744.97 |   74.61 | 1564.44 | 1862.32 |   1508.30 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 2773.45 | 2773.45 | 2773.45 |   0.00 | 2773.45 | 2773.45 | 2773.45 |   1484.52 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 | 1833.27 | 1761.12 | 1948.34 |  67.15 | 1796.50 | 1932.10 | 1945.09 |   1484.52 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 2786.76 | 2786.76 | 2786.76 |   0.00 | 2786.76 | 2786.76 | 2786.76 |   1462.95 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 | 1931.50 | 1904.15 | 1946.65 |  15.21 | 1933.07 | 1946.23 | 1946.57 |   1462.95 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 2832.76 | 2796.74 | 2894.54 |  33.03 | 2824.67 | 2881.96 | 2892.03 |   1394.92 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 2781.85 | 2781.85 | 2781.85 |   0.00 | 2781.85 | 2781.85 | 2781.85 |   1335.27 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  476.86 |  103.25 | 1968.50 | 745.82 |  104.09 | 1595.69 | 1893.94 |   1335.27 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 2835.57 | 2835.57 | 2835.57 |   0.00 | 2835.57 | 2835.57 | 2835.57 |   1368.83 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  476.83 |  105.23 | 1961.78 | 742.47 |  105.42 | 1590.69 | 1887.56 |   1368.83 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 2789.34 | 2789.34 | 2789.34 |   0.00 | 2789.34 | 2789.34 | 2789.34 |   1361.02 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  480.89 |  107.89 | 1969.59 | 744.35 |  109.02 | 1597.60 | 1895.19 |   1361.02 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 2825.52 | 2825.52 | 2825.52 |   0.00 | 2825.52 | 2825.52 | 2825.52 |   1306.55 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 | 1849.89 | 1804.74 | 1960.04 |  55.97 | 1826.42 | 1935.10 | 1955.05 |   1306.55 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 2844.18 | 2844.18 | 2844.18 |   0.00 | 2844.18 | 2844.18 | 2844.18 |   1352.86 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 | 1984.76 | 1947.54 | 2046.68 |  33.36 | 1973.05 | 2034.57 | 2044.26 |   1352.86 |
| HostPython                     | -          | -              | -                   |    5 |   14.38 |   12.69 |   19.95 |   2.80 |   13.07 |   18.62 |   19.69 |    247.14 |

#### Profile: low
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 2716.59 | 2706.01 | 2728.46 |   7.83 | 2713.79 | 2727.18 | 2728.20 |   1627.30 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 2805.59 | 2805.59 | 2805.59 |   0.00 | 2805.59 | 2805.59 | 2805.59 |   1326.77 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  441.93 |   74.50 | 1906.77 | 732.42 |   75.39 | 1541.02 | 1833.62 |   1326.77 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 2720.47 | 2720.47 | 2720.47 |   0.00 | 2720.47 | 2720.47 | 2720.47 |   1318.31 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  447.60 |   73.61 | 1941.76 | 747.08 |   74.13 | 1568.34 | 1867.08 |   1318.31 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 2720.16 | 2720.16 | 2720.16 |   0.00 | 2720.16 | 2720.16 | 2720.16 |   1382.31 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  438.89 |   73.28 | 1899.29 | 730.20 |   74.03 | 1534.32 | 1826.29 |   1382.31 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 2745.62 | 2745.62 | 2745.62 |   0.00 | 2745.62 | 2745.62 | 2745.62 |   1611.23 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 | 1798.38 | 1758.46 | 1906.92 |  54.68 | 1775.83 | 1881.00 | 1901.73 |   1611.23 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 2722.07 | 2722.07 | 2722.07 |   0.00 | 2722.07 | 2722.07 | 2722.07 |   1624.19 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 | 1906.34 | 1877.59 | 1916.86 |  14.54 | 1912.87 | 1916.34 | 1916.75 |   1624.19 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 2795.11 | 2735.09 | 2831.06 |  34.66 | 2815.50 | 2827.95 | 2830.44 |   1120.27 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 2718.34 | 2718.34 | 2718.34 |   0.00 | 2718.34 | 2718.34 | 2718.34 |   1324.17 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  454.47 |   88.97 | 1904.42 | 724.98 |   94.26 | 1542.64 | 1832.06 |   1324.17 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 2816.61 | 2816.61 | 2816.61 |   0.00 | 2816.61 | 2816.61 | 2816.61 |   1336.38 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  459.02 |   88.12 | 1937.14 | 739.06 |   89.74 | 1567.95 | 1863.30 |   1336.38 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 2755.48 | 2755.48 | 2755.48 |   0.00 | 2755.48 | 2755.48 | 2755.48 |   1330.19 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  465.01 |   90.72 | 1955.21 | 745.10 |   93.46 | 1582.89 | 1880.75 |   1330.19 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 2739.36 | 2739.36 | 2739.36 |   0.00 | 2739.36 | 2739.36 | 2739.36 |   1305.05 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 | 1819.11 | 1790.10 | 1897.07 |  39.97 | 1796.55 | 1880.90 | 1893.83 |   1305.05 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 2826.36 | 2826.36 | 2826.36 |   0.00 | 2826.36 | 2826.36 | 2826.36 |    990.25 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 | 1900.15 | 1887.51 | 1920.75 |  11.26 | 1895.38 | 1916.98 | 1920.00 |    990.25 |
| HostPython                     | -          | -              | -                   |    5 |    1.64 |    0.77 |    4.98 |   1.67 |    0.78 |    4.17 |    4.82 |     93.28 |

#### Profile: medium
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 2745.48 | 2733.61 | 2754.60 |   7.46 | 2748.97 | 2753.62 | 2754.40 |   1599.44 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 2760.81 | 2760.81 | 2760.81 |   0.00 | 2760.81 | 2760.81 | 2760.81 |   1572.95 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  436.74 |   73.09 | 1887.47 | 725.37 |   73.80 | 1525.18 | 1815.01 |   1572.95 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 2768.49 | 2768.49 | 2768.49 |   0.00 | 2768.49 | 2768.49 | 2768.49 |   1606.55 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  442.67 |   73.51 | 1916.70 | 737.01 |   74.12 | 1548.38 | 1843.03 |   1606.55 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 2724.82 | 2724.82 | 2724.82 |   0.00 | 2724.82 | 2724.82 | 2724.82 |   1580.61 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  443.28 |   74.49 | 1916.10 | 736.41 |   75.27 | 1547.96 | 1842.47 |   1580.61 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 2872.59 | 2872.59 | 2872.59 |   0.00 | 2872.59 | 2872.59 | 2872.59 |   1558.11 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 | 1823.29 | 1767.71 | 1910.30 |  51.03 | 1807.44 | 1897.65 | 1907.77 |   1558.11 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 2699.14 | 2699.14 | 2699.14 |   0.00 | 2699.14 | 2699.14 | 2699.14 |   1609.69 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 | 1888.98 | 1881.41 | 1896.80 |   6.33 | 1888.17 | 1896.57 | 1896.76 |   1609.69 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 2757.23 | 2731.11 | 2773.48 |  17.07 | 2764.70 | 2773.47 | 2773.48 |   1609.30 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 2762.52 | 2762.52 | 2762.52 |   0.00 | 2762.52 | 2762.52 | 2762.52 |   1568.89 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  454.51 |   86.22 | 1924.00 | 734.75 |   87.58 | 1556.72 | 1850.55 |   1568.89 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 2758.95 | 2758.95 | 2758.95 |   0.00 | 2758.95 | 2758.95 | 2758.95 |   1601.25 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  459.04 |   86.92 | 1939.32 | 740.14 |   90.66 | 1569.60 | 1865.38 |   1601.25 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 2768.42 | 2768.42 | 2768.42 |   0.00 | 2768.42 | 2768.42 | 2768.42 |   1592.22 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  457.32 |   89.07 | 1927.25 | 734.96 |   90.27 | 1559.97 | 1853.79 |   1592.22 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 2746.19 | 2746.19 | 2746.19 |   0.00 | 2746.19 | 2746.19 | 2746.19 |   1529.94 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 | 1806.90 | 1770.41 | 1906.42 |  50.35 | 1784.39 | 1883.94 | 1901.93 |   1529.94 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 2733.73 | 2733.73 | 2733.73 |   0.00 | 2733.73 | 2733.73 | 2733.73 |   1600.81 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 | 1912.97 | 1892.97 | 1929.13 |  13.76 | 1916.74 | 1928.24 | 1928.95 |   1600.81 |
| HostPython                     | -          | -              | -                   |    5 |    1.90 |    0.83 |    5.83 |   1.96 |    0.92 |    4.87 |    5.64 |     91.94 |

#### Profile: none
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 2749.83 | 2699.37 | 2838.91 |  48.39 | 2740.63 | 2821.92 | 2835.51 |   1065.44 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 2719.94 | 2719.94 | 2719.94 |   0.00 | 2719.94 | 2719.94 | 2719.94 |   1545.09 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  439.15 |   73.56 | 1898.50 | 729.68 |   74.31 | 1533.83 | 1825.57 |   1545.09 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 2696.44 | 2696.44 | 2696.44 |   0.00 | 2696.44 | 2696.44 | 2696.44 |   1552.69 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  440.14 |   72.29 | 1907.39 | 733.63 |   73.55 | 1540.80 | 1834.08 |   1552.69 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 2725.79 | 2725.79 | 2725.79 |   0.00 | 2725.79 | 2725.79 | 2725.79 |   1553.12 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  446.57 |   73.65 | 1933.60 | 743.52 |   75.07 | 1562.24 | 1859.33 |   1553.12 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 2739.51 | 2739.51 | 2739.51 |   0.00 | 2739.51 | 2739.51 | 2739.51 |   1550.72 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 | 1816.34 | 1760.99 | 1890.31 |  51.94 | 1792.31 | 1885.40 | 1889.33 |   1550.72 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 2749.15 | 2749.15 | 2749.15 |   0.00 | 2749.15 | 2749.15 | 2749.15 |   1234.44 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 | 1931.63 | 1896.53 | 1973.96 |  25.86 | 1932.93 | 1967.02 | 1972.57 |   1234.44 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 2746.26 | 2736.56 | 2762.94 |   9.12 | 2742.98 | 2759.96 | 2762.34 |   1559.47 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 2774.73 | 2774.73 | 2774.73 |   0.00 | 2774.73 | 2774.73 | 2774.73 |   1569.23 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  449.43 |   85.60 | 1896.34 | 723.46 |   89.41 | 1535.00 | 1824.07 |   1569.23 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 2742.42 | 2742.42 | 2742.42 |   0.00 | 2742.42 | 2742.42 | 2742.42 |   1568.72 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  463.39 |   87.76 | 1956.18 | 746.40 |   90.66 | 1583.47 | 1881.64 |   1568.72 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 2795.30 | 2795.30 | 2795.30 |   0.00 | 2795.30 | 2795.30 | 2795.30 |   1575.44 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  457.26 |   89.03 | 1915.56 | 729.16 |   90.59 | 1552.73 | 1842.99 |   1575.44 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 2764.24 | 2764.24 | 2764.24 |   0.00 | 2764.24 | 2764.24 | 2764.24 |   1567.73 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 | 1825.75 | 1782.32 | 1947.44 |  63.27 | 1786.82 | 1923.71 | 1942.69 |   1567.73 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 2733.29 | 2733.29 | 2733.29 |   0.00 | 2733.29 | 2733.29 | 2733.29 |   1561.34 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 | 1910.81 | 1899.21 | 1933.18 |  12.02 | 1909.82 | 1928.61 | 1932.26 |   1561.34 |
| HostPython                     | -          | -              | -                   |    5 |    1.84 |    0.77 |    5.93 |   2.04 |    0.82 |    4.92 |    5.73 |     92.59 |


### Tensor
#### Profile: high
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1616.77 | 1593.41 | 1662.11 |  23.99 | 1608.05 | 1653.21 | 1660.33 |   1366.53 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1617.20 | 1617.20 | 1617.20 |   0.00 | 1617.20 | 1617.20 | 1617.20 |   1495.42 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  452.51 |  338.68 |  863.19 | 205.63 |  346.99 |  764.60 |  843.47 |   1495.42 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1604.18 | 1604.18 | 1604.18 |   0.00 | 1604.18 | 1604.18 | 1604.18 |   1536.20 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  450.29 |  339.39 |  855.32 | 202.78 |  345.45 |  757.98 |  835.86 |   1536.20 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1619.81 | 1619.81 | 1619.81 |   0.00 | 1619.81 | 1619.81 | 1619.81 |   1501.52 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  454.83 |  335.22 |  867.70 | 207.04 |  346.60 |  770.41 |  848.24 |   1501.52 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1624.27 | 1624.27 | 1624.27 |   0.00 | 1624.27 | 1624.27 | 1624.27 |   1469.20 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  778.98 |  746.46 |  864.84 |  43.66 |  757.20 |  846.13 |  861.10 |   1469.20 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1596.31 | 1596.31 | 1596.31 |   0.00 | 1596.31 | 1596.31 | 1596.31 |   1424.30 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  875.88 |  848.88 |  890.63 |  16.02 |  886.79 |  889.90 |  890.49 |   1424.30 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1235.97 | 1216.83 | 1264.28 |  17.40 | 1226.76 | 1260.91 | 1263.61 |   1526.53 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1241.85 | 1241.85 | 1241.85 |   0.00 | 1241.85 | 1241.85 | 1241.85 |   1563.20 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  123.86 |   34.74 |  473.74 | 174.94 |   37.39 |  386.55 |  456.30 |   1563.20 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1223.46 | 1223.46 | 1223.46 |   0.00 | 1223.46 | 1223.46 | 1223.46 |   1591.39 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  126.98 |   35.26 |  488.24 | 180.63 |   36.53 |  398.32 |  470.26 |   1591.39 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1237.05 | 1237.05 | 1237.05 |   0.00 | 1237.05 | 1237.05 | 1237.05 |   1577.41 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  126.18 |   34.99 |  482.31 | 178.07 |   37.10 |  393.89 |  464.62 |   1577.41 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1246.90 | 1246.90 | 1246.90 |   0.00 | 1246.90 | 1246.90 | 1246.90 |   1554.16 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  420.12 |  405.53 |  463.09 |  21.61 |  409.96 |  453.05 |  461.08 |   1554.16 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1236.85 | 1236.85 | 1236.85 |   0.00 | 1236.85 | 1236.85 | 1236.85 |   1540.14 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  485.21 |  475.45 |  513.39 |  14.17 |  479.62 |  506.64 |  512.04 |   1540.14 |
| HostPython                     | -          | -              | -                   |    5 |    0.00 |    0.00 |    0.01 |   0.00 |    0.00 |    0.01 |    0.01 |     46.05 |

#### Profile: low
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1221.46 | 1198.21 | 1245.63 |  19.57 | 1221.76 | 1244.64 | 1245.43 |   1342.67 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1227.67 | 1227.67 | 1227.67 |   0.00 | 1227.67 | 1227.67 | 1227.67 |   1478.81 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  108.26 |   19.82 |  459.73 | 175.74 |   20.24 |  372.05 |  442.20 |   1478.81 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1204.54 | 1204.54 | 1204.54 |   0.00 | 1204.54 | 1204.54 | 1204.54 |   1481.64 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  105.94 |   19.75 |  448.24 | 171.15 |   20.25 |  362.88 |  431.17 |   1481.64 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1210.09 | 1210.09 | 1210.09 |   0.00 | 1210.09 | 1210.09 | 1210.09 |   1489.84 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  105.79 |   19.59 |  446.54 | 170.38 |   20.68 |  361.62 |  429.56 |   1489.84 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1237.97 | 1237.97 | 1237.97 |   0.00 | 1237.97 | 1237.97 | 1237.97 |   1477.94 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  403.24 |  382.04 |  451.58 |  24.89 |  392.26 |  441.39 |  449.54 |   1477.94 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1224.69 | 1224.69 | 1224.69 |   0.00 | 1224.69 | 1224.69 | 1224.69 |   1370.03 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  457.70 |  454.30 |  462.94 |   2.89 |  457.38 |  461.92 |  462.74 |   1370.03 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1217.76 | 1208.76 | 1224.66 |   5.67 | 1216.44 | 1224.33 | 1224.59 |   1484.84 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1406.29 | 1406.29 | 1406.29 |   0.00 | 1406.29 | 1406.29 | 1406.29 |   1491.95 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  117.93 |   31.97 |  460.31 | 171.19 |   32.06 |  374.91 |  443.23 |   1491.95 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1222.61 | 1222.61 | 1222.61 |   0.00 | 1222.61 | 1222.61 | 1222.61 |   1489.41 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  120.22 |   31.41 |  469.64 | 174.71 |   33.10 |  382.63 |  452.24 |   1489.41 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1225.78 | 1225.78 | 1225.78 |   0.00 | 1225.78 | 1225.78 | 1225.78 |   1488.59 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  122.66 |   33.94 |  474.03 | 175.69 |   35.26 |  386.31 |  456.49 |   1488.59 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1237.43 | 1237.43 | 1237.43 |   0.00 | 1237.43 | 1237.43 | 1237.43 |   1497.73 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  416.44 |  402.08 |  463.33 |  23.53 |  406.14 |  452.19 |  461.10 |   1497.73 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1244.31 | 1244.31 | 1244.31 |   0.00 | 1244.31 | 1244.31 | 1244.31 |   1484.39 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  468.67 |  460.75 |  485.71 |   9.29 |  464.97 |  482.76 |  485.12 |   1484.39 |
| HostPython                     | -          | -              | -                   |    5 |    0.00 |    0.00 |    0.01 |   0.00 |    0.00 |    0.01 |    0.01 |     36.25 |

#### Profile: medium
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1271.34 | 1243.83 | 1307.14 |  20.34 | 1269.10 | 1299.74 | 1305.66 |   1605.33 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1270.07 | 1270.07 | 1270.07 |   0.00 | 1270.07 | 1270.07 | 1270.07 |   1606.47 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  130.86 |   41.10 |  483.73 | 176.44 |   43.20 |  395.70 |  466.13 |   1606.47 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1251.96 | 1251.96 | 1251.96 |   0.00 | 1251.96 | 1251.96 | 1251.96 |   1584.81 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  136.03 |   40.31 |  508.49 | 186.24 |   43.91 |  416.01 |  489.99 |   1584.81 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1259.74 | 1259.74 | 1259.74 |   0.00 | 1259.74 | 1259.74 | 1259.74 |   1581.02 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  130.13 |   40.02 |  483.52 | 176.70 |   41.27 |  395.78 |  465.97 |   1581.02 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1239.18 | 1239.18 | 1239.18 |   0.00 | 1239.18 | 1239.18 | 1239.18 |   1612.61 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  435.16 |  411.27 |  494.70 |  30.60 |  419.73 |  482.35 |  492.23 |   1612.61 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1275.70 | 1275.70 | 1275.70 |   0.00 | 1275.70 | 1275.70 | 1275.70 |   1608.56 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  478.80 |  469.24 |  495.74 |   9.46 |  475.43 |  492.95 |  495.18 |   1608.56 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1249.74 | 1217.86 | 1282.12 |  23.16 | 1250.83 | 1278.99 | 1281.49 |   1575.09 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1242.14 | 1242.14 | 1242.14 |   0.00 | 1242.14 | 1242.14 | 1242.14 |   1567.83 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  124.52 |   32.60 |  488.22 | 181.86 |   33.18 |  397.75 |  470.13 |   1567.83 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1272.39 | 1272.39 | 1272.39 |   0.00 | 1272.39 | 1272.39 | 1272.39 |   1565.11 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  121.12 |   33.06 |  471.08 | 174.98 |   33.75 |  383.76 |  453.62 |   1565.11 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1231.91 | 1231.91 | 1231.91 |   0.00 | 1231.91 | 1231.91 | 1231.91 |   1574.62 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  124.83 |   32.93 |  489.26 | 182.22 |   33.59 |  398.45 |  471.10 |   1574.62 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1250.24 | 1250.24 | 1250.24 |   0.00 | 1250.24 | 1250.24 | 1250.24 |   1571.44 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  434.10 |  407.87 |  475.15 |  22.43 |  428.45 |  467.20 |  473.56 |   1571.44 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1241.64 | 1241.64 | 1241.64 |   0.00 | 1241.64 | 1241.64 | 1241.64 |   1600.19 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  475.23 |  469.01 |  485.96 |   5.96 |  474.08 |  484.06 |  485.58 |   1600.19 |
| HostPython                     | -          | -              | -                   |    5 |    0.00 |    0.00 |    0.01 |   0.00 |    0.00 |    0.01 |    0.01 |     37.64 |

#### Profile: none
| Mode                           | Invocation | Path           | Cleanup             | Iter |  Avg ms |  Min ms |  Max ms | Std ms |  P50 ms |  P95 ms |  P99 ms | RSS (MiB) |
| ------------------------------ | ---------- | -------------- | ------------------- | ---: | ------: | ------: | ------: | -----: | ------: | ------: | ------: | --------: |
| AardvarkJsonCold               | json       | cold           | -                   |    5 | 1211.02 | 1199.42 | 1221.51 |   8.42 | 1207.43 | 1221.19 | 1221.44 |   1579.69 |
| AardvarkJsonPersistent         | json       | first-call     | full                |    1 | 1251.33 | 1251.33 | 1251.33 |   0.00 | 1251.33 | 1251.33 | 1251.33 |   1590.86 |
| AardvarkJsonPersistent         | json       | persistent     | full                |    5 |  109.31 |   21.63 |  456.69 | 173.69 |   22.67 |  370.01 |  439.35 |   1590.86 |
| AardvarkJsonPersistentNone     | json       | first-call     | none                |    1 | 1240.48 | 1240.48 | 1240.48 |   0.00 | 1240.48 | 1240.48 | 1240.48 |   1579.53 |
| AardvarkJsonPersistentNone     | json       | persistent     | none                |    5 |  113.84 |   19.90 |  484.05 | 185.11 |   21.75 |  391.84 |  465.61 |   1579.53 |
| AardvarkJsonPersistentShared   | json       | first-call     | shared-buffers-only |    1 | 1247.05 | 1247.05 | 1247.05 |   0.00 | 1247.05 | 1247.05 | 1247.05 |   1583.92 |
| AardvarkJsonPersistentShared   | json       | persistent     | shared-buffers-only |    5 |  106.48 |   19.75 |  449.81 | 171.67 |   20.57 |  364.30 |  432.71 |   1583.92 |
| AardvarkJsonResetInPlace       | json       | first-call     | -                   |    1 | 1205.16 | 1205.16 | 1205.16 |   0.00 | 1205.16 | 1205.16 | 1205.16 |   1589.55 |
| AardvarkJsonResetInPlace       | json       | reset-in-place | -                   |    5 |  406.30 |  390.02 |  442.59 |  19.11 |  399.64 |  435.48 |  441.17 |   1589.55 |
| AardvarkJsonWarm               | json       | first-call     | -                   |    1 | 1201.96 | 1201.96 | 1201.96 |   0.00 | 1201.96 | 1201.96 | 1201.96 |   1585.14 |
| AardvarkJsonWarm               | json       | warm           | -                   |    5 |  449.86 |  446.97 |  451.67 |   1.78 |  450.47 |  451.63 |  451.66 |   1585.14 |
| AardvarkRawCtxCold             | raw-ctx    | cold           | -                   |    5 | 1258.66 | 1240.78 | 1275.27 |  11.75 | 1257.29 | 1273.54 | 1274.92 |   1587.84 |
| AardvarkRawCtxPersistent       | raw-ctx    | first-call     | full                |    1 | 1235.05 | 1235.05 | 1235.05 |   0.00 | 1235.05 | 1235.05 | 1235.05 |   1588.88 |
| AardvarkRawCtxPersistent       | raw-ctx    | persistent     | full                |    5 |  121.79 |   31.82 |  478.21 | 178.21 |   32.80 |  389.37 |  460.44 |   1588.88 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | first-call     | none                |    1 | 1207.57 | 1207.57 | 1207.57 |   0.00 | 1207.57 | 1207.57 | 1207.57 |   1607.30 |
| AardvarkRawCtxPersistentNone   | raw-ctx    | persistent     | none                |    5 |  122.89 |   33.04 |  476.59 | 176.85 |   34.98 |  388.37 |  458.94 |   1607.30 |
| AardvarkRawCtxPersistentShared | raw-ctx    | first-call     | shared-buffers-only |    1 | 1235.91 | 1235.91 | 1235.91 |   0.00 | 1235.91 | 1235.91 | 1235.91 |   1598.61 |
| AardvarkRawCtxPersistentShared | raw-ctx    | persistent     | shared-buffers-only |    5 |  119.69 |   31.52 |  469.35 | 174.83 |   32.29 |  382.15 |  451.91 |   1598.61 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | first-call     | -                   |    1 | 1223.89 | 1223.89 | 1223.89 |   0.00 | 1223.89 | 1223.89 | 1223.89 |   1588.95 |
| AardvarkRawCtxResetInPlace     | raw-ctx    | reset-in-place | -                   |    5 |  423.98 |  404.91 |  471.51 |  24.34 |  416.73 |  461.00 |  469.41 |   1588.95 |
| AardvarkRawCtxWarm             | raw-ctx    | first-call     | -                   |    1 | 1242.76 | 1242.76 | 1242.76 |   0.00 | 1242.76 | 1242.76 | 1242.76 |   1588.67 |
| AardvarkRawCtxWarm             | raw-ctx    | warm           | -                   |    5 |  467.27 |  464.77 |  471.37 |   2.35 |  467.47 |  470.61 |  471.22 |   1588.67 |
| HostPython                     | -          | -              | -                   |    5 |    0.00 |    0.00 |    0.01 |   0.00 |    0.00 |    0.01 |    0.01 |     36.64 |


