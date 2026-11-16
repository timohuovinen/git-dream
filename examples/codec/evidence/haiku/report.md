best score after n attempts (mean ± 95% interval over repeats; start 121673; every run has at least 48 attempts)
| arm | repeats | 12 attempts | 24 attempts | 36 attempts | 48 attempts |
|---|---:|---:|---:|---:|---:|
| bandit | 4 | 36588 ± 1351 | 34942 ± 1580 | 33930 ± 1008 | 33518 ± 1305 |
| chains | 4 | 36881 ± 2453 | 34947 ± 880 | 34234 ± 1175 | 33914 ± 1209 |
| fixed | 4 | 36475 ± 1743 | 34711 ± 1182 | 33972 ± 1342 | 33556 ± 1278 |
| paper | 4 | 36580 ± 1612 | 35233 ± 466 | 34616 ± 629 | 34027 ± 841 |

final score against each other arm (row minus column, mean difference ± 95% Welch interval; below zero: the row is better)
| | bandit | chains | fixed | paper |
|---|---:|---:|---:|---:|
| bandit | — | -396 ± 1437 | -37 ± 1476 | -508 ± 1254 |
| chains | +396 ± 1437 | — | +359 ± 1421 | -112 ± 1190 |
| fixed | +37 ± 1476 | -359 ± 1421 | — | -471 ± 1236 |
| paper | +508 ± 1254 | +112 ± 1190 | +471 ± 1236 | — |

per run
| arm | final scores, sorted | failed | unchanged | agent cost | agent minutes | tokens in (M) | tokens out (M) |
|---|---|---:|---:|---:|---:|---:|---:|
| bandit | 32804 32922 33792 34556 | 0% | 9% | $4.26 | 389 | 113.2 | 3.34 |
| chains | 33030 33637 34182 34809 | 0% | 11% | $3.49 | 313 | 87.6 | 2.68 |
| fixed | 32876 33249 33380 34717 | 0% | 8% | $4.22 | 385 | 108.3 | 3.40 |
| paper | 33553 33595 34386 34573 | 0% | 15% | $3.85 | 356 | 98.0 | 3.02 |

replay support of the first 24 recorded attempts of each run (strict replay, 24 attempts per candidate, refine_best + 200 sampled policies; candidates may stop after 4 attempts)
| arm | recorded by | pool's mean miss rate | pool in support | replay winner's miss rate | replay winner's attempts | winner is not refine_best |
|---|---|---:|---:|---:|---:|---:|
| bandit | bandit(min_calls=4) | 60% | 4% | 25% | 9.8 | 4/4 |
| chains | chains(w=4,d=6,min_calls=4) | 45% | 16% | 0% | 7.5 | 4/4 |
| fixed | refine_best(min_calls=4) | 68% | 1% | 41% | 17.0 | 4/4 |
| paper | chains(w=4,d=6,min_calls=4) | 47% | 15% | 0% | 5.0 | 4/4 |

replay support of the first 24 recorded attempts of each run (strict replay, 24 attempts per candidate, refine_best + 200 sampled policies; candidates spend the whole budget)
| arm | recorded by | pool's mean miss rate | pool in support | replay winner's miss rate | replay winner's attempts | winner is not refine_best |
|---|---|---:|---:|---:|---:|---:|
| bandit | bandit(min_calls=4) | 77% | 0% | 51% | 24.0 | 4/4 |
| chains | chains(w=4,d=6,min_calls=4) | 70% | 2% | 44% | 24.0 | 3/4 |
| fixed | refine_best(min_calls=4) | 80% | 0% | 64% | 24.0 | 4/4 |
| paper | chains(w=4,d=6,min_calls=4) | 71% | 1% | 34% | 24.0 | 4/4 |

rounds (policy deployed, attempts spent)
  bandit-1: bandit(min_calls=4) 48
  bandit-2: bandit(min_calls=4) 48
  bandit-3: bandit(min_calls=4) 48
  bandit-4: bandit(min_calls=4) 48
  chains-1: chains(w=4,d=6,min_calls=4) 24; chains(w=4,d=6,min_calls=4) 24
  chains-2: chains(w=4,d=6,min_calls=4) 24; chains(w=4,d=6,min_calls=4) 24
  chains-3: chains(w=4,d=6,min_calls=4) 24; chains(w=4,d=6,min_calls=4) 24
  chains-4: chains(w=4,d=6,min_calls=4) 24; chains(w=4,d=6,min_calls=4) 24
  fixed-1: refine_best(min_calls=4) 48
  fixed-2: refine_best(min_calls=4) 48
  fixed-3: refine_best(min_calls=4) 48
  fixed-4: refine_best(min_calls=4) 48
  paper-1: chains(w=4,d=6,min_calls=4) 24; chains(w=1,d=6,min_calls=4) 24
  paper-2: chains(w=4,d=6,min_calls=4) 24; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4
  paper-3: chains(w=4,d=6,min_calls=4) 24; chains(w=2,d=1,min_calls=4) 4; chains(w=2,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4; chains(w=4,d=1,min_calls=4) 4
  paper-4: chains(w=4,d=6,min_calls=4) 24; chains(w=1,d=6,min_calls=4) 24
