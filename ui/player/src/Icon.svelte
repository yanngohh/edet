<script>
  export let name;
  export let size = 18;
  export let stroke = 1.7;
  const paths = {
    network: "M12 3 3 8v9l9 4 9-4V8L12 3Zm0 0v9m-9-4 9 4 9-4m-9 4v9",
    chart: "M4 3v17h17M8 15l4-5 4 3 5-7",
    shield: "M12 3 4 6v6c0 5 8 9 8 9s8-4 8-9V6l-8-3Zm-4 9 3 3 5-6",
    folder: "M3 7V5h6l2 2h10v13H3V7Zm0 3h18",
    expand: "M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5",
    close: "m6 6 12 12M6 18 18 6",
    play: "m9 5 11 7-11 7V5Z",
    pause: "M8 5v14M16 5v14",
    previous: "M5 5v14m14-14L8 12l11 7V5Z",
    next: "M19 5v14M5 5l11 7-11 7V5Z",
    arrow: "M5 12h14m-5-5 5 5-5 5",
    arrowup: "M5 16 16 5M6 5h10v10",
    people:
      "M16 21v-3a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v3m16-7a4 4 0 0 1 4 4v3M9 3a4 4 0 1 0 0 8 4 4 0 0 0 0-8m8 0a4 4 0 0 1 0 8",
    link: "m10 13 4-4m-6 6-2 2a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0m4 2 2-2a4 4 0 0 0-6-6l-4 4a4 4 0 0 0 0 6",
    activity: "M2 12h5l3-8 4 16 3-8h5",
    globe: "M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0ZM3 12h18M12 3c-5 5-5 13 0 18 5-5 5-13 0-18Z",
    reset: "M3 10a9 9 0 1 1 2 8M3 4v6h6",
    plus: "M12 5v14M5 12h14",
    minus: "M5 12h14",
    info: "M12 11v6m0-10v.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z",
    chevron: "m9 5 7 7-7 7",
    search: "M10.5 3a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15ZM16 16l5 5",
    check: "m5 12 4 4L19 6",
    spark: "m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5L12 3Z",
  };
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width={stroke} stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name] || paths.network} /></svg>
