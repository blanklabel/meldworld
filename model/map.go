package model

type WorldMap struct {
	Type string `json:"type"`
	MapObj
	EntityObj
}

// Size of a map
type Dimension struct {
	Height int
	Width  int
}

// Container for the jason
type MapObj struct {
	Map Dimension
}
