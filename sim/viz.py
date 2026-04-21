import csv
import os

def generate_svg(csv_path, output_path, title, x_col, line_configs):
    """
    Generates a robust, light-themed SVG plot.
    line_configs: list of dicts {'col': str, 'label': str, 'color': str, 'dash': str}
    """
    if not os.path.exists(csv_path):
        print(f"Error: {csv_path} not found.")
        return

    data = []
    with open(csv_path, 'r') as f:
        reader = csv.DictReader(f)
        for row in reader:
            try:
                data.append({k: float(v) for k, v in row.items()})
            except (ValueError, TypeError):
                continue

    if not data:
        print(f"Error: No valid data in {csv_path}")
        return

    # Dimensions
    w, h, p_left, p_right, p_top, p_bottom = 800, 450, 60, 160, 60, 50
    
    max_x = max(d[x_col] for d in data)
    min_x = min(d[x_col] for d in data)
    if max_x == min_x: max_x += 1.0

    y_vals = []
    for cfg in line_configs:
        y_vals.extend([d[cfg['col']] for d in data])
    
    max_y = max(y_vals)
    min_y = min(y_vals)
    # Add some breathing room for Y axis
    max_y = max_y + (max_y - min_y) * 0.1 if max_y != min_y else max_y + 1.0
    min_y = min_y - (max_y - min_y) * 0.1 if max_y != min_y else min_y - 0.1

    def sx(x): return p_left + (x - min_x) / (max_x - min_x) * (w - p_left - p_right)
    def sy(y): return h - p_bottom - (y - min_y) / (max_y - min_y) * (h - p_top - p_bottom)

    svg = [
        '<?xml version="1.0" encoding="UTF-8" standalone="no"?>',
        f'<svg width="{w}" height="{h}" viewBox="0 0 {w} {h}" xmlns="http://www.w3.org/2000/svg">',
        '  <rect width="100%" height="100%" fill="#ffffff"/>',
        f'  <text x="{w/2}" y="35" text-anchor="middle" font-size="20" font-family="Arial" font-weight="bold" fill="#333">{title}</text>'
    ]

    # Grid lines (Y axis)
    for i in range(5):
        y_val = min_y + i * (max_y - min_y) / 4
        y_pos = sy(y_val)
        svg.append(f'  <text x="{p_left-10}" y="{y_pos+5}" text-anchor="end" font-size="12" fill="#999" font-family="Arial">{y_val:.2f}</text>')
        svg.append(f'  <line x1="{p_left}" y1="{y_pos}" x2="{w-p_right}" y2="{y_pos}" stroke="#eee" stroke-width="1"/>')

    # Data lines
    for cfg in line_configs:
        pts = " ".join([f"{sx(d[x_col]):.1f},{sy(d[cfg['col']]):.1f}" for d in data])
        dash = f' stroke-dasharray="{cfg["dash"]}"' if cfg.get('dash') else ''
        svg.append(f'  <polyline points="{pts}" fill="none" stroke="{cfg["color"]}" stroke-width="2.5"{dash}/>')

    # Legend
    legend_x = w - p_right + 10
    svg.append(f'  <rect x="{legend_x}" y="{p_top}" width="{p_right-20}" height="{len(line_configs)*25 + 10}" fill="#fafafa" stroke="#ddd" rx="3"/>')
    for i, cfg in enumerate(line_configs):
        ly = p_top + 20 + i * 25
        dash = f' stroke-dasharray="{cfg["dash"]}"' if cfg.get('dash') else ''
        svg.append(f'  <line x1="{legend_x+10}" y1="{ly-5}" x2="{legend_x+40}" y2="{ly-5}" stroke="{cfg["color"]}" stroke-width="3"{dash}/>')
        svg.append(f'  <text x="{legend_x+45}" y="{ly}" font-size="12" font-family="Arial" fill="#666">{cfg["label"]}</text>')

    # Special annotations for Mixed Scenario
    if "mixed" in csv_path:
        annotations = [(20, "Gateway"), (40, "Sybil"), (60, "Slacker")]
        for x_val, label in annotations:
            xp = sx(x_val)
            svg.append(f'  <line x1="{xp:.1f}" y1="{h-p_bottom}" x2="{xp:.1f}" y2="{p_top}" stroke="#f00" stroke-width="1" stroke-dasharray="2,2"/>')
            svg.append(f'  <text x="{xp+2:.1f}" y="{p_top-5}" fill="#f66" font-size="10" font-family="Arial" transform="rotate(-45 {xp} {p_top-10})">{label}</text>')

    svg.append('</svg>')
    
    with os.fdopen(os.open(output_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644), 'w') as f:
        f.write("\n".join(svg))
    print(f"Generated {output_path}")
