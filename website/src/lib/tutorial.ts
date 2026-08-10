/**
 * The tutorial, in reading order.
 *
 * Unlike the reference, none of this is generated: the tutorial is prose
 * somebody wrote, and its order is an argument about what to learn first. This
 * table is that order, and the only place it is written down — the sidebar, the
 * index, and the prev/next links at the foot of a page all read it, so adding a
 * chapter is a page under `src/pages/tutorial/` and one entry here.
 */
export interface TutorialPage {
	/** The last segment of the URL: `/tutorial/<slug>/`. */
	slug: string;
	title: string;
	/** A sentence for the index — what the chapter is for, not what it covers. */
	blurb: string;
}

export const TUTORIAL: TutorialPage[] = [
	{
		slug: "getting-started",
		title: "Getting started",
		blurb: "Make a sound, quiet it down, and wrap it in a function you can play.",
	},
	{
		slug: "make-some-noise",
		title: "Make some noise",
		blurb: "Make a sound, quiet it down, and wrap it in a function you can play.",
	},
];

export function pageBySlug(slug: string): TutorialPage | undefined {
	return TUTORIAL.find((p) => p.slug === slug);
}

/** The chapters either side of one, for the links at the foot of a page. */
export function neighbors(slug: string): {
	prev?: TutorialPage;
	next?: TutorialPage;
} {
	const i = TUTORIAL.findIndex((p) => p.slug === slug);
	if (i === -1) return {};
	return { prev: TUTORIAL[i - 1], next: TUTORIAL[i + 1] };
}
